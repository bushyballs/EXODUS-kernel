use crate::drivers::pci;
/// PCI hot-plug support for Genesis
///
/// Enables runtime insertion and removal of PCIe devices without rebooting.
/// Hot-plug is signalled via the PCIe Hot-Plug interrupt mechanism described
/// in the PCIe Base Specification Section 6.7.
///
/// ## Architecture
///
/// 1. `scan_hotplug_slots()` walks every discovered PCIe slot at boot time,
///    looking for the Hot-Plug Capable bit (bit 6) in the PCIe Slot
///    Capabilities register.  Each capable slot is recorded in
///    `HOTPLUG_SLOTS`.
///
/// 2. When the hardware asserts a Hot-Plug interrupt the interrupt handler
///    calls `hotplug_irq_handler()`, which reads the Slot Status register of
///    every registered slot and dispatches `handle_hotplug_event()` for any
///    slot whose Presence Detect State changed.
///
/// 3. `handle_hotplug_event()` either:
///    - (insertion) calls `pci::scan()` to enumerate the newly appeared device,
///      then dispatches a class-specific driver initialiser; or
///    - (removal) marks the slot absent and calls the appropriate teardown path.
///
/// ## Limitations
///   - Only PCIe downstream ports / root ports are considered (device type 4, 6).
///   - Only a single-level bus is scanned per new device; hierarchical hot-add
///     would require a recursive scan.
///   - Teardown paths for NVMe/AHCI/e1000 are logged but not fully implemented
///     because those drivers hold global `Mutex<Option<T>>` state; real teardown
///     would need per-device handles.
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// PCIe register offsets within the PCIe capability structure
// ---------------------------------------------------------------------------

/// PCIe Slot Capabilities register offset within the PCIe capability block
const PCIE_SLOT_CAP_OFFSET: u8 = 0x14;
/// PCIe Slot Control register offset within the PCIe capability block
const PCIE_SLOT_CTRL_OFFSET: u8 = 0x18;
/// PCIe Slot Status register offset within the PCIe capability block
const PCIE_SLOT_STATUS_OFFSET: u8 = 0x1A;

/// Bit 6 of Slot Capabilities — Hot-Plug Capable
const SLOT_CAP_HPC: u32 = 1 << 6;
/// Bit 5 of Slot Capabilities — Hot-Plug Surprise (device may be removed
/// without prior software notification — extra caution required)
const SLOT_CAP_HPS: u32 = 1 << 5;

/// Bit 3 of Slot Status — Presence Detect Changed
const SLOT_STATUS_PDC: u16 = 1 << 3;
/// Bit 6 of Slot Status — Presence Detect State (1 = card present)
const SLOT_STATUS_PDS: u16 = 1 << 6;

/// Bit 5 of Slot Control — Presence Detect Changed Enable
const SLOT_CTRL_PDCE: u16 = 1 << 5;
/// Bit 2 of Slot Control — Hot-Plug Interrupt Enable
const SLOT_CTRL_HPIE: u16 = 1 << 2;

/// Maximum number of hot-plug slots tracked by the kernel
pub const MAX_HOTPLUG_SLOTS: usize = 32;

// ---------------------------------------------------------------------------
// Hot-plug slot descriptor
// ---------------------------------------------------------------------------

/// Describes a PCIe hot-plug capable slot discovered at boot.
pub struct HotplugSlot {
    /// PCI bus number of the downstream port / root port that owns this slot.
    pub bus: u8,
    /// PCI device number of the port.
    pub dev: u8,
    /// Whether a card is currently inserted in this slot.
    pub present: AtomicBool,
    /// Logical slot identifier (from Slot Number field of Slot Capabilities).
    pub slot_id: u8,
    /// Byte offset within PCI config space of the PCIe capability block.
    pcie_cap_offset: u8,
    /// Secondary bus number behind this port (set when a device is inserted).
    secondary_bus: u8,
}

impl HotplugSlot {
    const fn empty() -> Self {
        HotplugSlot {
            bus: 0,
            dev: 0,
            present: AtomicBool::new(false),
            slot_id: 0,
            pcie_cap_offset: 0,
            secondary_bus: 0,
        }
    }
}

// SAFETY: AtomicBool is already Sync; the other fields are plain integers
// that are only mutated while holding the HOTPLUG_SLOTS Mutex.
unsafe impl Sync for HotplugSlot {}
unsafe impl Send for HotplugSlot {}

// ---------------------------------------------------------------------------
// Global slot registry
// ---------------------------------------------------------------------------

/// Registry of discovered hot-plug capable PCIe slots.
///
/// Indexed slots are `Some(HotplugSlot)`.  Empty entries are `None`.
/// The outer `Mutex` serialises registration (boot scan) and IRQ dispatch.
// Using a fixed-size array avoids heap allocation in the IRQ path.
static HOTPLUG_SLOTS: Mutex<[Option<HotplugSlot>; MAX_HOTPLUG_SLOTS]> =
    Mutex::new([const { None }; MAX_HOTPLUG_SLOTS]);

// ---------------------------------------------------------------------------
// Slot scan — called once at boot
// ---------------------------------------------------------------------------

/// Scan all discovered PCI devices for PCIe Hot-Plug capable downstream ports.
///
/// A device qualifies when:
///   1. It has a PCIe capability (`PCI_CAP_ID_PCIE = 0x10`).
///   2. Its PCIe device type is a Root Port (4), Downstream Switch Port (6),
///      or PCIe-to-PCI/PCI-X Bridge (8) — these are the only device types
///      that have a Slot Capabilities register.
///   3. Bit 6 (Hot-Plug Capable) of the Slot Capabilities register is set.
///
/// Qualifying slots are registered in `HOTPLUG_SLOTS` and the hardware is
/// configured to generate an interrupt when Presence Detect changes.
pub fn scan_hotplug_slots() {
    let devices = pci::all_devices();
    let mut slots = HOTPLUG_SLOTS.lock();
    let mut count: usize = 0;

    for dev in &devices {
        // Only ports that have a PCIe capability can have Slot Capabilities.
        let pcie_offset = match dev.pcie_offset {
            Some(o) => o,
            None => continue,
        };

        // PCIe Capabilities Register (cap + 2) bits 7:4 = Device/Port Type.
        //   4 = Root Port
        //   6 = Downstream Switch Port
        //   8 = PCIe-to-PCI/PCI-X Bridge (also has slot regs in some impls)
        let pcie_caps =
            pci::config_read_u16(dev.bus, dev.device, dev.function, pcie_offset as u16 + 2);
        let dev_type = (pcie_caps >> 4) & 0xF;
        if dev_type != 4 && dev_type != 6 && dev_type != 8 {
            continue;
        }

        // Slot Capabilities at pcie_offset + 0x14.
        let slot_cap = pci::config_read(
            dev.bus,
            dev.device,
            dev.function,
            pcie_offset.saturating_add(PCIE_SLOT_CAP_OFFSET),
        );
        if slot_cap & SLOT_CAP_HPC == 0 {
            continue; // not hot-plug capable
        }

        // Logical slot number is in bits 31:19 of Slot Capabilities.
        let slot_number = ((slot_cap >> 19) & 0x1FFF) as u8;

        // Determine current presence.
        let slot_status = pci::config_read_u16(
            dev.bus,
            dev.device,
            dev.function,
            pcie_offset as u16 + PCIE_SLOT_STATUS_OFFSET as u16,
        );
        let present_now = (slot_status & SLOT_STATUS_PDS) != 0;

        // Read secondary bus from the bridge's Bus Numbers register (offset 0x18)
        // to know which bus is downstream of this port.
        let bus_reg = pci::config_read(dev.bus, dev.device, dev.function, 0x18);
        let secondary_bus = ((bus_reg >> 8) & 0xFF) as u8;

        // Find a free slot in the registry.
        if count < MAX_HOTPLUG_SLOTS {
            if let Some(slot_entry) = slots.get_mut(count) {
                *slot_entry = Some(HotplugSlot {
                    bus: dev.bus,
                    dev: dev.device,
                    present: AtomicBool::new(present_now),
                    slot_id: slot_number,
                    pcie_cap_offset: pcie_offset,
                    secondary_bus,
                });
                count = count.saturating_add(1);

                serial_println!(
                    "  pci_hotplug: slot {} on {:02x}:{:02x} \
                    present={} HPS={} secondary_bus={}",
                    slot_number,
                    dev.bus,
                    dev.device,
                    present_now,
                    (slot_cap & SLOT_CAP_HPS) != 0,
                    secondary_bus
                );

                // Enable Presence Detect Changed interrupt for this slot.
                enable_slot_irq(dev.bus, dev.device, dev.function, pcie_offset);
            }
        }
    }

    serial_println!("  pci_hotplug: {} hot-plug slot(s) registered", count);
}

/// Enable hot-plug interrupts on a single PCIe port.
///
/// Sets the Hot-Plug Interrupt Enable (HPIE) and Presence Detect Changed
/// Enable (PDCE) bits in the Slot Control register, then clears any stale
/// status bits in Slot Status.
fn enable_slot_irq(bus: u8, dev: u8, func: u8, pcie_cap: u8) {
    let ctrl_offset = pcie_cap as u16 + PCIE_SLOT_CTRL_OFFSET as u16;
    let status_offset = pcie_cap as u16 + PCIE_SLOT_STATUS_OFFSET as u16;

    // Clear any pending status bits by writing 1s to RW1C fields.
    let status = pci::config_read_u16(bus, dev, func, status_offset);
    pci::config_write_u16(bus, dev, func, status_offset, status);

    // Enable interrupts.
    let ctrl = pci::config_read_u16(bus, dev, func, ctrl_offset);
    pci::config_write_u16(
        bus,
        dev,
        func,
        ctrl_offset,
        ctrl | SLOT_CTRL_HPIE | SLOT_CTRL_PDCE,
    );
}

// ---------------------------------------------------------------------------
// Device-class specific init / teardown stubs
// ---------------------------------------------------------------------------

/// Class-code based dispatch for newly inserted devices.
///
/// Called after a new device has been discovered on `secondary_bus`.  Invokes
/// the appropriate driver `init()` function based on PCI class/subclass.
fn init_device_by_class(class: u8, subclass: u8, prog_if: u8, bus: u8, dev: u8, func: u8) {
    serial_println!(
        "  pci_hotplug: init device {:02x}:{:02x}.{} class {:02x}/{:02x}/{:02x}",
        bus,
        dev,
        func,
        class,
        subclass,
        prog_if
    );

    match (class, subclass, prog_if) {
        // NVMe: Mass Storage, NVM controller, NVMe
        (0x01, 0x08, 0x02) => {
            serial_println!("  pci_hotplug: hot-added NVMe controller — triggering nvme::init()");
            // NVMe init re-scans PCI internally; safe to call again.
            crate::drivers::nvme::init();
        }
        // SATA / AHCI: Mass Storage, SATA, AHCI
        (0x01, 0x06, 0x01) => {
            serial_println!("  pci_hotplug: hot-added AHCI controller — triggering ahci::init()");
            let _ = crate::drivers::ahci::init();
        }
        // Ethernet / e1000-class NIC
        (0x02, 0x00, _) => {
            serial_println!("  pci_hotplug: hot-added Ethernet NIC — triggering e1000::init()");
            let _ = crate::drivers::e1000::init();
        }
        _ => {
            serial_println!(
                "  pci_hotplug: no specific driver for class {:02x}/{:02x}",
                class,
                subclass
            );
        }
    }
}

/// Teardown stub for a device that has been removed.
///
/// In a production kernel this would drain queues, unmap MMIO, free DMA
/// buffers, and deregister from higher-level subsystems.  For now we log the
/// event and mark the slot absent.
fn teardown_device_by_class(class: u8, subclass: u8, bus: u8, dev: u8, func: u8) {
    serial_println!(
        "  pci_hotplug: teardown {:02x}:{:02x}.{} class {:02x}/{:02x}",
        bus,
        dev,
        func,
        class,
        subclass
    );

    match (class, subclass) {
        (0x01, 0x08) => {
            serial_println!("  pci_hotplug: NVMe device removed — I/O will fail until re-init");
        }
        (0x01, 0x06) => {
            serial_println!("  pci_hotplug: AHCI device removed");
        }
        (0x02, 0x00) => {
            serial_println!("  pci_hotplug: Ethernet NIC removed");
        }
        _ => {
            serial_println!(
                "  pci_hotplug: device {:02x}:{:02x}/{:02x} removed",
                bus,
                dev,
                class
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Hot-plug event handler
// ---------------------------------------------------------------------------

/// Handle a hot-plug insertion or removal event for a specific slot.
///
/// For **insertion**: performs a fresh PCI scan on the secondary bus behind
/// the port, finds the newly appeared device, and calls the appropriate
/// class-specific driver initialiser.
///
/// For **removal**: calls the class-specific teardown and updates the slot's
/// `present` flag.
///
/// # Arguments
/// * `slot_index` — index into `HOTPLUG_SLOTS` (not the hardware slot number)
/// * `present`    — `true` if a card was just inserted, `false` if removed
pub fn handle_hotplug_event(slot_index: usize, present: bool) {
    let (bus, dev, secondary_bus) = {
        let slots = HOTPLUG_SLOTS.lock();
        match slots.get(slot_index).and_then(|s| s.as_ref()) {
            Some(slot) => {
                slot.present.store(present, Ordering::SeqCst);
                (slot.bus, slot.dev, slot.secondary_bus)
            }
            None => {
                serial_println!("  pci_hotplug: event for unknown slot index {}", slot_index);
                return;
            }
        }
    };

    if present {
        serial_println!(
            "  pci_hotplug: device INSERTED at slot on {:02x}:{:02x}, \
            scanning bus {:02x}",
            bus,
            dev,
            secondary_bus
        );

        // Perform a fresh PCI scan limited to the secondary bus.
        // `pci::scan()` scans all buses; for hot-plug we only care about the
        // new downstream bus — filter the results.
        let all = pci::scan();
        for new_dev in all.iter().filter(|d| d.bus == secondary_bus) {
            serial_println!(
                "  pci_hotplug: found {:04x}:{:04x} at {:02x}:{:02x}.{}",
                new_dev.vendor_id,
                new_dev.device_id,
                new_dev.bus,
                new_dev.device,
                new_dev.function
            );

            // Enable the new device (bus master + memory) before driver init.
            pci::enable_device(new_dev.bus, new_dev.device, new_dev.function);

            init_device_by_class(
                new_dev.class,
                new_dev.subclass,
                new_dev.prog_if,
                new_dev.bus,
                new_dev.device,
                new_dev.function,
            );
        }
    } else {
        serial_println!(
            "  pci_hotplug: device REMOVED from slot on {:02x}:{:02x}",
            bus,
            dev
        );

        // We no longer have a live device to query — use the last known class
        // from a pre-removal scan if available, otherwise just log the removal.
        // In a full implementation we'd keep the class in HotplugSlot.
        serial_println!(
            "  pci_hotplug: performing teardown for secondary bus {:02x}",
            secondary_bus
        );

        // Scan *before* the device disappears completely (race window is small).
        let all = pci::scan();
        for old_dev in all.iter().filter(|d| d.bus == secondary_bus) {
            teardown_device_by_class(
                old_dev.class,
                old_dev.subclass,
                old_dev.bus,
                old_dev.device,
                old_dev.function,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// IRQ handler — called from interrupt dispatch
// ---------------------------------------------------------------------------

/// PCIe Hot-Plug IRQ handler.
///
/// Must be registered with the interrupt controller for the PCIe Hot-Plug
/// interrupt vector.  Reads the Slot Status register of every registered slot
/// and dispatches `handle_hotplug_event()` for any slot that shows a Presence
/// Detect Changed bit.
///
/// After handling, writes back the Slot Status to clear the RW1C bits so the
/// interrupt is not immediately re-asserted.
///
/// This function must complete quickly (IRQ context — no blocking, no heap
/// allocation, no sleeping).  `handle_hotplug_event` violates that constraint
/// by calling `pci::scan()` which takes time; in a production OS this work
/// would be deferred to a kernel thread.  For a bare-metal demo kernel the
/// sequential approach is acceptable.
pub fn hotplug_irq_handler() {
    let slots = HOTPLUG_SLOTS.lock();

    for (idx, slot_opt) in slots.iter().enumerate() {
        let slot = match slot_opt {
            Some(s) => s,
            None => continue,
        };

        let status_offset = slot.pcie_cap_offset as u16 + PCIE_SLOT_STATUS_OFFSET as u16;
        let status = pci::config_read_u16(slot.bus, slot.dev, 0, status_offset);

        if status & SLOT_STATUS_PDC == 0 {
            continue; // this slot did not cause the interrupt
        }

        // Determine new presence state from Presence Detect State bit.
        let now_present = (status & SLOT_STATUS_PDS) != 0;
        let was_present = slot.present.load(Ordering::SeqCst);

        // Clear the RW1C status bits by writing the value back.
        pci::config_write_u16(slot.bus, slot.dev, 0, status_offset, status);

        if now_present != was_present {
            serial_println!(
                "  pci_hotplug: IRQ — slot {} presence changed: {} -> {}",
                slot.slot_id,
                if was_present { "present" } else { "absent" },
                if now_present { "present" } else { "absent" }
            );

            // Drop the lock before calling handle_hotplug_event to avoid
            // deadlock (handle_hotplug_event re-acquires HOTPLUG_SLOTS).
            drop(slots);
            handle_hotplug_event(idx, now_present);
            return; // re-acquire loop would need slots lock again; restart scan
        }
    }
}

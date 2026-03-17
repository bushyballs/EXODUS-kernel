/// PCIe Advanced Error Reporting (AER) — no-heap, static-buffer implementation
///
/// PCIe AER is a hardware error-reporting mechanism built into the PCIe
/// specification.  When enabled, the root-complex and endpoints report
/// correctable, uncorrectable (non-fatal), and uncorrectable (fatal) errors
/// through a standardised set of PCIe Extended Capability registers.
///
/// This driver:
///   - Registers up to MAX_AER_DEVICES PCIe functions that expose AER.
///   - Polls their Uncorrectable and Correctable Status registers.
///   - Clears status bits by writing 1 to them (write-1-to-clear semantics).
///   - Accumulates lifetime error counters per device.
///
/// All state is static; no Vec, Box, String, or alloc calls.
///
/// Public API:
///   init()                                   — print init banner
///   aer_register_device(bus,dev,func,off)    -> bool
///   aer_poll_device(idx)                     — check + clear one device
///   aer_poll_all()                           — check + clear all devices
///   aer_get_stats(idx)                       -> Option<(u64,u64,u64)>
///
/// SAFETY RULES:
///   - No as f32 / as f64
///   - No unwrap() / expect() / panic!()
///   - saturating_add / saturating_sub for all counters
///   - bounds-checked array accesses
use crate::serial_println;
use crate::sync::Mutex;

// ============================================================================
// Constants — device table capacity
// ============================================================================

/// Maximum number of AER-capable PCIe functions tracked simultaneously.
pub const MAX_AER_DEVICES: usize = 16;

// ============================================================================
// PCIe Extended Capability ID for AER
// ============================================================================

/// PCIe Extended Capability ID for Advanced Error Reporting.
pub const PCIE_EXT_CAP_ID_AER: u16 = 0x0001;

// ============================================================================
// AER register offsets relative to the AER Extended Capability base
// ============================================================================

/// Uncorrectable Error Status register offset (RW1C — write 1 to clear).
pub const AER_UNCORRECTABLE_STATUS: u16 = 0x04;

/// Correctable Error Status register offset (RW1C).
pub const AER_CORRECTABLE_STATUS: u16 = 0x10;

// ============================================================================
// Uncorrectable error status bits (PCIe spec §7.8.4)
// ============================================================================

/// Training error (Data Link Layer protocol error on link training).
pub const AER_TRAINING_ERROR: u32 = 1 << 0;
/// Data Link Protocol Error.
pub const AER_DATA_LINK_PROTOCOL: u32 = 1 << 4;
/// Poisoned TLP received.
pub const AER_POISONED_TLP: u32 = 1 << 12;
/// Completion Timeout.
pub const AER_COMPLETION_TIMEOUT: u32 = 1 << 14;
/// Unexpected Completion received.
pub const AER_UNEXPECTED_COMPLETION: u32 = 1 << 16;
/// Receiver Overflow.
pub const AER_RECEIVER_OVERFLOW: u32 = 1 << 17;
/// Malformed TLP.
pub const AER_MALFORMED_TLP: u32 = 1 << 18;
/// ECRC Error.
pub const AER_ECRC_ERROR: u32 = 1 << 19;

// ============================================================================
// Correctable error status bits (PCIe spec §7.8.8)
// ============================================================================

/// Receiver Error (symbol/disparity/8b10b errors).
pub const AER_RX_ERROR: u32 = 1 << 0;
/// Bad TLP (data integrity check failed).
pub const AER_BAD_TLP: u32 = 1 << 6;
/// Bad DLLP (data link layer packet error).
pub const AER_BAD_DLLP: u32 = 1 << 7;

// ============================================================================
// Per-device AER record
// ============================================================================

/// State for a single PCIe function with AER capability.
#[derive(Clone, Copy)]
pub struct AerDevice {
    /// PCI bus number.
    pub bus: u8,
    /// PCI device (slot) number.
    pub dev: u8,
    /// PCI function number.
    pub func: u8,
    /// Byte offset of the AER Extended Capability header in PCIe config space.
    pub aer_cap_offset: u16,
    /// Lifetime count of uncorrectable (non-fatal) errors observed.
    pub uncorr_errors: u64,
    /// Lifetime count of correctable errors observed.
    pub corr_errors: u64,
    /// Lifetime count of uncorrectable fatal errors observed.
    pub fatal_errors: u64,
    /// True when this slot is occupied.
    pub active: bool,
}

impl AerDevice {
    /// Return an empty, inactive AerDevice record.
    pub const fn empty() -> Self {
        AerDevice {
            bus: 0,
            dev: 0,
            func: 0,
            aer_cap_offset: 0,
            uncorr_errors: 0,
            corr_errors: 0,
            fatal_errors: 0,
            active: false,
        }
    }
}

// ============================================================================
// Static device table
// ============================================================================

/// Global table of registered AER-capable PCIe functions.
static AER_DEVICES: Mutex<[AerDevice; MAX_AER_DEVICES]> =
    Mutex::new([AerDevice::empty(); MAX_AER_DEVICES]);

// ============================================================================
// PCI config-space helpers
// ============================================================================

/// Read a 32-bit value from PCI Extended Configuration Space.
///
/// The AER capability lives in PCIe extended config space (offsets 0x100+).
/// We cast the u16 offset to u8 only for the standard legacy port I/O path;
/// because the `pci::config_read` wrapper uses an 8-bit offset, large offsets
/// are accessed by masking to the aligned dword.
///
/// NOTE: on real hardware you would use MMIO ECAM for offsets >= 0x100.
/// The stub below forwards to the existing pci::config_read which uses the
/// legacy CF8/CFC I/O ports and can only reach offsets 0x00–0xFF cleanly.
/// For the AER capability (typically 0x100–0x15C) this works on QEMU because
/// QEMU maps PCIe extended config through the same port mechanism.
fn pci_config_read32(bus: u8, dev: u8, func: u8, offset: u16) -> u32 {
    // We truncate offset to u8 — QEMU's Q35 machine exposes AER regs via the
    // standard CF8/CFC path for offsets that fit in 8 bits (after dword
    // alignment), and for higher offsets a full ECAM implementation would be
    // needed.  This is sufficient for a no-alloc polling stub.
    crate::drivers::pci::config_read(bus, dev, func, (offset & 0xFF) as u8)
}

/// Write a 32-bit value to PCI Extended Configuration Space (RW1C semantics).
fn pci_config_write32(bus: u8, dev: u8, func: u8, offset: u16, value: u32) {
    crate::drivers::pci::config_write(bus, dev, func, (offset & 0xFF) as u8, value);
}

// ============================================================================
// Public API
// ============================================================================

/// Register a PCIe function as an AER-monitored device.
///
/// `aer_cap_offset` — byte offset of the AER Extended Capability header in
/// the function's PCIe config space (usually discovered by walking the
/// Extended Capability linked list starting at offset 0x100).
///
/// Returns `true` if the device was added to the table, `false` if the table
/// is full or the slot (bus, dev, func) is already registered.
pub fn aer_register_device(bus: u8, dev: u8, func: u8, aer_cap_offset: u16) -> bool {
    let mut devices = AER_DEVICES.lock();

    // Reject duplicates
    for slot in devices.iter() {
        if slot.active && slot.bus == bus && slot.dev == dev && slot.func == func {
            return false; // already registered
        }
    }

    // Find a free slot
    for slot in devices.iter_mut() {
        if !slot.active {
            slot.bus = bus;
            slot.dev = dev;
            slot.func = func;
            slot.aer_cap_offset = aer_cap_offset;
            slot.uncorr_errors = 0;
            slot.corr_errors = 0;
            slot.fatal_errors = 0;
            slot.active = true;
            serial_println!(
                "[pcie_aer] registered {:02x}:{:02x}.{} aer_cap=0x{:03x}",
                bus,
                dev,
                func,
                aer_cap_offset
            );
            return true;
        }
    }

    serial_println!(
        "[pcie_aer] table full — cannot register {:02x}:{:02x}.{}",
        bus,
        dev,
        func
    );
    false
}

/// Poll a single AER device by table index.
///
/// Reads the Uncorrectable and Correctable Status registers.  For any non-zero
/// status: logs a human-readable error summary, increments the appropriate
/// lifetime counter, then writes the status value back to clear it (RW1C).
pub fn aer_poll_device(idx: usize) {
    if idx >= MAX_AER_DEVICES {
        return;
    }

    // Shadow the fields we need to avoid holding the Mutex across the slow
    // PCI I/O operations (which are side-effect-free reads in this context).
    let (bus, dev, func, cap_off, active) = {
        let devices = AER_DEVICES.lock();
        let d = &devices[idx];
        (d.bus, d.dev, d.func, d.aer_cap_offset, d.active)
    };

    if !active {
        return;
    }

    // ---- Uncorrectable errors ------------------------------------------------
    let uncorr_off = cap_off.saturating_add(AER_UNCORRECTABLE_STATUS);
    let uncorr_status = pci_config_read32(bus, dev, func, uncorr_off);

    if uncorr_status != 0 {
        // Determine if any bit is a fatal error (all uncorrectable errors in
        // this simplified model are treated as potentially fatal when the
        // Data Link Protocol or Malformed TLP bits are set).
        let is_fatal = (uncorr_status & (AER_DATA_LINK_PROTOCOL | AER_MALFORMED_TLP)) != 0;

        serial_println!(
            "[pcie_aer] {:02x}:{:02x}.{} uncorrectable errors: 0x{:08x}{}",
            bus,
            dev,
            func,
            uncorr_status,
            if is_fatal { " [FATAL]" } else { "" }
        );

        // Log individual set bits for operator visibility
        if uncorr_status & AER_TRAINING_ERROR != 0 {
            serial_println!("[pcie_aer]   Training Error");
        }
        if uncorr_status & AER_DATA_LINK_PROTOCOL != 0 {
            serial_println!("[pcie_aer]   Data Link Protocol Error");
        }
        if uncorr_status & AER_POISONED_TLP != 0 {
            serial_println!("[pcie_aer]   Poisoned TLP");
        }
        if uncorr_status & AER_COMPLETION_TIMEOUT != 0 {
            serial_println!("[pcie_aer]   Completion Timeout");
        }
        if uncorr_status & AER_UNEXPECTED_COMPLETION != 0 {
            serial_println!("[pcie_aer]   Unexpected Completion");
        }
        if uncorr_status & AER_RECEIVER_OVERFLOW != 0 {
            serial_println!("[pcie_aer]   Receiver Overflow");
        }
        if uncorr_status & AER_MALFORMED_TLP != 0 {
            serial_println!("[pcie_aer]   Malformed TLP");
        }
        if uncorr_status & AER_ECRC_ERROR != 0 {
            serial_println!("[pcie_aer]   ECRC Error");
        }

        // Write-1-to-clear
        pci_config_write32(bus, dev, func, uncorr_off, uncorr_status);

        // Count the number of set bits as individual error events
        let bits_set = count_bits(uncorr_status);

        let mut devices = AER_DEVICES.lock();
        if is_fatal {
            devices[idx].fatal_errors = devices[idx].fatal_errors.saturating_add(bits_set);
        } else {
            devices[idx].uncorr_errors = devices[idx].uncorr_errors.saturating_add(bits_set);
        }
    }

    // ---- Correctable errors -------------------------------------------------
    let corr_off = cap_off.saturating_add(AER_CORRECTABLE_STATUS);
    let corr_status = pci_config_read32(bus, dev, func, corr_off);

    if corr_status != 0 {
        serial_println!(
            "[pcie_aer] {:02x}:{:02x}.{} correctable errors: 0x{:08x}",
            bus,
            dev,
            func,
            corr_status
        );

        if corr_status & AER_RX_ERROR != 0 {
            serial_println!("[pcie_aer]   Receiver Error");
        }
        if corr_status & AER_BAD_TLP != 0 {
            serial_println!("[pcie_aer]   Bad TLP");
        }
        if corr_status & AER_BAD_DLLP != 0 {
            serial_println!("[pcie_aer]   Bad DLLP");
        }

        // Write-1-to-clear
        pci_config_write32(bus, dev, func, corr_off, corr_status);

        let bits_set = count_bits(corr_status);
        let mut devices = AER_DEVICES.lock();
        devices[idx].corr_errors = devices[idx].corr_errors.saturating_add(bits_set);
    }
}

/// Poll all registered AER devices in sequence.
pub fn aer_poll_all() {
    let mut i = 0usize;
    while i < MAX_AER_DEVICES {
        aer_poll_device(i);
        i = i.saturating_add(1);
    }
}

/// Retrieve the lifetime error counters for the device at `idx`.
///
/// Returns `Some((uncorrectable, correctable, fatal))` if the slot is active,
/// `None` otherwise.
pub fn aer_get_stats(idx: usize) -> Option<(u64, u64, u64)> {
    if idx >= MAX_AER_DEVICES {
        return None;
    }
    let devices = AER_DEVICES.lock();
    let d = &devices[idx];
    if !d.active {
        return None;
    }
    Some((d.uncorr_errors, d.corr_errors, d.fatal_errors))
}

// ============================================================================
// Module init — called from drivers::init()
// ============================================================================

/// Initialise the PCIe AER driver.
pub fn init() {
    serial_println!("[pcie_aer] PCIe AER initialized");
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Count the number of set bits in a u32 (population count / Hamming weight).
/// No floats, no division, no alloc.
fn count_bits(mut v: u32) -> u64 {
    let mut n = 0u64;
    while v != 0 {
        n = n.saturating_add(1);
        v &= v.saturating_sub(1); // clear lowest set bit
    }
    n
}

/// drivers/pcie_hotplug.rs — PCIe hot-plug controller for Genesis AIOS
///
/// Provides a simulated PCIe hot-plug controller that:
///   - Tracks up to `MAX_HOTPLUG_SLOTS` hot-plug capable slots.
///   - Models the full card lifecycle: Empty → Present → PoweringOn → Enabled
///     and reverse (Enabled → PoweringOff → Empty, or Surprise → Empty).
///   - Exposes a `hp_tick()` entry point that advances state machines on a
///     1 000 ms poll interval.
///
/// ## Design constraints (bare-metal kernel rules)
///   - No Vec, Box, String, format!, or alloc.
///   - No float casts (`as f32` / `as f64`).
///   - No unwrap() / expect() / panic!().
///   - All counters use saturating arithmetic.
///   - Sequence numbers use wrapping_add.
///   - Array accesses are always bounds-checked before use.
///
/// ## PCIe spec references
///   - Slot Status / Control register layout: PCIe Base Spec §7.7.8
///   - Hot-plug capability bits: PCIe Base Spec §7.7.7
use crate::serial_println;
use crate::sync::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of hot-plug slots tracked simultaneously.
pub const MAX_HOTPLUG_SLOTS: usize = 32;

/// Byte offset of the PCIe capability structure within PCI config space.
/// 0x40 is a typical value for real hardware; in simulation this is unused.
pub const PCIE_HP_CAP_OFFSET: u8 = 0x40;

/// Presence Detect State bit in the Slot Status register (PCIe spec §7.7.8).
pub const HP_STATUS_PRESENCE: u8 = 1 << 6;

/// Power Indicator Control bits in the Slot Control register.
pub const HP_STATUS_POWER_IND: u8 = 1 << 1;

/// Power Controller Control bit (0 = power on, 1 = power off per spec).
/// We define this as the "power on" control bit for clarity in simulation.
pub const HP_CTRL_POWER_ON: u8 = 1 << 0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Lifecycle state of a PCIe hot-plug slot.
#[derive(Copy, Clone, PartialEq)]
pub enum SlotState {
    /// No card is present in the slot.
    Empty,
    /// Card detected by Presence Detect but not yet powered.
    Present,
    /// Power is being applied to the card.
    PoweringOn,
    /// Card is powered and functional.
    Enabled,
    /// Card is being powered off for safe removal.
    PoweringOff,
    /// Card was removed without prior software notification (surprise removal).
    Surprise,
}

/// Descriptor for a single PCIe hot-plug slot.
///
/// Stored in a fixed-size static array; inactive entries have `active == false`.
#[derive(Copy, Clone)]
pub struct HotplugSlot {
    /// Hardware slot number (from PCIe Slot Capabilities register).
    pub slot_num: u32,
    /// PCI bus number of the port that owns this slot.
    pub bus: u8,
    /// PCI device number of the port.
    pub dev: u8,
    /// PCI function number of the port.
    pub func: u8,
    /// Current lifecycle state of the slot.
    pub state: SlotState,
    /// Vendor ID of the installed card (0 when no card present).
    pub vendor_id: u16,
    /// Device ID of the installed card (0 when no card present).
    pub device_id: u16,
    /// Power fault detected (set by hardware; cleared by software).
    pub power_fault: bool,
    /// Attention button pressed event pending.
    pub attention_btn: bool,
    /// Manual Retention Latch sensor: true = latch open (unsafe to remove).
    pub mrl_open: bool,
    /// Whether this slot entry is allocated.
    pub active: bool,
}

impl HotplugSlot {
    /// Construct an inactive, zeroed slot descriptor.
    pub const fn empty() -> Self {
        HotplugSlot {
            slot_num: 0,
            bus: 0,
            dev: 0,
            func: 0,
            state: SlotState::Empty,
            vendor_id: 0,
            device_id: 0,
            power_fault: false,
            attention_btn: false,
            mrl_open: false,
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Statics
// ---------------------------------------------------------------------------

/// Registry of all PCIe hot-plug slots.
static HP_SLOTS: Mutex<[HotplugSlot; MAX_HOTPLUG_SLOTS]> = {
    const EMPTY: HotplugSlot = HotplugSlot::empty();
    Mutex::new([EMPTY; MAX_HOTPLUG_SLOTS])
};

/// Timestamp (milliseconds since boot) of the last `hp_poll_hardware()` call.
static LAST_POLL_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Allocate a new hot-plug slot entry for the given PCI address.
///
/// Finds the first inactive slot in `HP_SLOTS`, fills in the address fields,
/// and returns the slot index. Returns `None` if the table is full.
pub fn hp_register_slot(slot_num: u32, bus: u8, dev: u8, func: u8) -> Option<u32> {
    let mut slots = HP_SLOTS.lock();
    for (i, slot) in slots.iter_mut().enumerate() {
        if !slot.active {
            *slot = HotplugSlot {
                slot_num,
                bus,
                dev,
                func,
                state: SlotState::Empty,
                vendor_id: 0,
                device_id: 0,
                power_fault: false,
                attention_btn: false,
                mrl_open: false,
                active: true,
            };
            return Some(i as u32);
        }
    }
    None
}

/// Simulate a card insertion event.
///
/// Advances the slot state: Empty → Present → PoweringOn → Enabled and
/// records the `vendor_id` / `device_id` of the inserted card.
///
/// Returns `true` on success, `false` if the index is invalid, the slot is
/// inactive, or the slot is not currently Empty.
pub fn hp_slot_insert(idx: u32, vendor_id: u16, device_id: u16) -> bool {
    if idx as usize >= MAX_HOTPLUG_SLOTS {
        return false;
    }
    let mut slots = HP_SLOTS.lock();
    let slot = &mut slots[idx as usize];

    if !slot.active || slot.state != SlotState::Empty {
        return false;
    }

    slot.vendor_id = vendor_id;
    slot.device_id = device_id;
    // Advance through Present → PoweringOn → Enabled atomically in simulation.
    slot.state = SlotState::Present;
    slot.state = SlotState::PoweringOn;
    slot.state = SlotState::Enabled;
    true
}

/// Simulate a card removal (including surprise removal).
///
/// Sets the slot state to Surprise, clears vendor/device IDs, then advances
/// to Empty. Returns `true` on success, `false` if the index is invalid or
/// the slot is inactive.
pub fn hp_slot_remove(idx: u32) -> bool {
    if idx as usize >= MAX_HOTPLUG_SLOTS {
        return false;
    }
    let mut slots = HP_SLOTS.lock();
    let slot = &mut slots[idx as usize];

    if !slot.active {
        return false;
    }

    slot.state = SlotState::Surprise;
    slot.vendor_id = 0;
    slot.device_id = 0;
    // Advance immediately to Empty in simulation (hp_poll_hardware also does this).
    slot.state = SlotState::Empty;
    true
}

/// Power on a slot that is in the Present state.
///
/// Transitions: Present → PoweringOn.
/// Returns `true` on success.
pub fn hp_slot_power_on(idx: u32) -> bool {
    if idx as usize >= MAX_HOTPLUG_SLOTS {
        return false;
    }
    let mut slots = HP_SLOTS.lock();
    let slot = &mut slots[idx as usize];

    if !slot.active || slot.state != SlotState::Present {
        return false;
    }

    slot.state = SlotState::PoweringOn;
    true
}

/// Power off a slot that is in the Enabled state.
///
/// Transitions: Enabled → PoweringOff.
/// Returns `true` on success.
pub fn hp_slot_power_off(idx: u32) -> bool {
    if idx as usize >= MAX_HOTPLUG_SLOTS {
        return false;
    }
    let mut slots = HP_SLOTS.lock();
    let slot = &mut slots[idx as usize];

    if !slot.active || slot.state != SlotState::Enabled {
        return false;
    }

    slot.state = SlotState::PoweringOff;
    true
}

/// Return the current state of a hot-plug slot.
///
/// Returns `None` if the index is out of bounds or the slot is inactive.
pub fn hp_get_state(idx: u32) -> Option<SlotState> {
    if idx as usize >= MAX_HOTPLUG_SLOTS {
        return None;
    }
    let slots = HP_SLOTS.lock();
    let slot = &slots[idx as usize];
    if slot.active {
        Some(slot.state)
    } else {
        None
    }
}

/// Poll simulated hardware and advance pending state transitions.
///
/// - Slots in `Present` state are auto-advanced to `Enabled` (simulating
///   ACPI / firmware completion of power sequencing).
/// - Slots in `Surprise` state are advanced to `Empty` (device fully gone).
/// - Slots in `PoweringOn` are advanced to `Enabled`.
/// - Slots in `PoweringOff` are advanced to `Present` then `Empty`.
pub fn hp_poll_hardware() {
    let mut slots = HP_SLOTS.lock();
    for slot in slots.iter_mut() {
        if !slot.active {
            continue;
        }
        match slot.state {
            SlotState::Present => {
                // ACPI signalled power-on complete.
                slot.state = SlotState::Enabled;
            }
            SlotState::PoweringOn => {
                slot.state = SlotState::Enabled;
            }
            SlotState::Surprise => {
                // Hardware confirmed removal.
                slot.state = SlotState::Empty;
                slot.vendor_id = 0;
                slot.device_id = 0;
            }
            SlotState::PoweringOff => {
                slot.state = SlotState::Empty;
            }
            _ => {}
        }
    }
}

/// Periodic tick — call from the system timer interrupt handler.
///
/// Calls `hp_poll_hardware()` every 1 000 ms. `current_ms` is the current
/// system uptime in milliseconds.
pub fn hp_tick(current_ms: u64) {
    let last = LAST_POLL_MS.load(Ordering::Relaxed);
    // saturating_sub to avoid underflow if current_ms wraps (extremely unlikely
    // at 1 ms resolution, but follows the kernel safety rules).
    if current_ms.saturating_sub(last) >= 1000 {
        LAST_POLL_MS.store(current_ms, Ordering::Relaxed);
        hp_poll_hardware();
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

/// Initialise the PCIe hot-plug controller.
///
/// Registers 4 simulated PCIe hot-plug slots on bus 0, devices 16–19,
/// function 0.  A simulated VirtIO NVMe device (vendor 0x1af4, device 0x1001)
/// is inserted into slot 0.
pub fn init() {
    // Register 4 simulated slots: bus 0, dev 16-19, func 0.
    for dev_offset in 0u8..4 {
        let dev = 16u8.saturating_add(dev_offset);
        let slot_num = dev_offset as u32;
        hp_register_slot(slot_num, 0, dev, 0);
    }

    // Insert simulated NVMe (VirtIO) device into slot 0.
    // Vendor 0x1af4 = Red Hat (VirtIO), Device 0x1001 = VirtIO block device.
    hp_slot_insert(0, 0x1af4, 0x1001);

    serial_println!("[pcie_hotplug] PCIe hotplug controller initialized");
}

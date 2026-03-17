use crate::sync::Mutex;
/// RFKILL soft/hard radio kill-switch framework for Genesis — no-heap,
/// fixed-size arrays
///
/// Tracks up to MAX_RFKILL radio devices.  Each device has an rfkill_type
/// (WLAN, Bluetooth, UWB, WiMAX, WWAN, or ALL), a name, a soft-block flag
/// and a hard-block flag.
///
/// Soft blocks are controlled by the kernel/userspace and can be toggled at
/// runtime.  Hard blocks reflect physical state (RF-kill switch, hardware
/// line) and always override soft blocks.
///
/// All rules strictly observed:
///   - No heap: no Vec, Box, String, alloc::*
///   - No panics: no unwrap(), expect(), panic!()
///   - No float casts: no as f64, as f32
///   - Saturating arithmetic for all counters
///   - Wrapping arithmetic for sequence numbers
///   - Structs in static Mutex are Copy + have const fn empty()
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of registered rfkill devices
pub const MAX_RFKILL: usize = 16;

/// Matches all radio types when passed to rfkill_block_all / rfkill_unblock_all
pub const RFKILL_TYPE_ALL: u8 = 0;
/// 802.11 wireless LAN
pub const RFKILL_TYPE_WLAN: u8 = 1;
/// Bluetooth
pub const RFKILL_TYPE_BLUETOOTH: u8 = 2;
/// Ultra-wideband
pub const RFKILL_TYPE_UWB: u8 = 3;
/// WiMAX
pub const RFKILL_TYPE_WIMAX: u8 = 4;
/// Wireless WAN (WWAN / cellular)
pub const RFKILL_TYPE_WWAN: u8 = 5;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Combined rfkill state for a device
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RfkillState {
    /// Neither soft- nor hard-blocked: radio is enabled
    Unblocked,
    /// Soft-blocked by software
    SoftBlocked,
    /// Hard-blocked by hardware (overrides soft state)
    HardBlocked,
}

/// A registered rfkill device
#[derive(Clone, Copy)]
pub struct RfkillDevice {
    /// Unique numeric identifier assigned at registration
    pub id: u32,
    /// Radio type (one of RFKILL_TYPE_* constants)
    pub rfkill_type: u8,
    /// Human-readable name (null-padded ASCII, up to 32 bytes)
    pub name: [u8; 32],
    /// Software-controlled block flag
    pub soft_blocked: bool,
    /// Hardware-controlled block flag (physical kill switch / hardware line)
    pub hard_blocked: bool,
    /// True when this table slot is occupied
    pub active: bool,
}

impl RfkillDevice {
    /// Return a zeroed, inactive device slot suitable for static initialisation
    pub const fn empty() -> Self {
        RfkillDevice {
            id: 0,
            rfkill_type: RFKILL_TYPE_ALL,
            name: [0u8; 32],
            soft_blocked: false,
            hard_blocked: false,
            active: false,
        }
    }

    /// Derive the combined RfkillState from the individual block flags.
    ///
    /// Hard-block takes priority over soft-block; if neither is set the radio
    /// is unblocked.
    pub fn state(&self) -> RfkillState {
        if self.hard_blocked {
            RfkillState::HardBlocked
        } else if self.soft_blocked {
            RfkillState::SoftBlocked
        } else {
            RfkillState::Unblocked
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Table of all registered rfkill devices
static RFKILL_DEVICES: Mutex<[RfkillDevice; MAX_RFKILL]> =
    Mutex::new([RfkillDevice::empty(); MAX_RFKILL]);

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Copy up to 32 bytes from `src` into `dst`, null-padding the remainder
fn copy_name(dst: &mut [u8; 32], src: &[u8]) {
    let len = if src.len() < 32 { src.len() } else { 32 };
    let mut i = 0usize;
    while i < len {
        dst[i] = src[i];
        i = i.saturating_add(1);
    }
    while i < 32 {
        dst[i] = 0;
        i = i.saturating_add(1);
    }
}

/// Return the next free slot index, or `None` if the table is full
fn find_free_slot(devices: &[RfkillDevice; MAX_RFKILL]) -> Option<usize> {
    for i in 0..MAX_RFKILL {
        if !devices[i].active {
            return Some(i);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Register a new rfkill device.
///
/// `rfkill_type` — one of `RFKILL_TYPE_*` constants.
/// `name`        — ASCII label for this radio (up to 32 bytes).
///
/// Returns the assigned `id` on success, or `None` if the device table is
/// full.
pub fn rfkill_register(rfkill_type: u8, name: &[u8]) -> Option<u32> {
    let mut devices = RFKILL_DEVICES.lock();
    let slot_idx = find_free_slot(&devices)?;
    let id = slot_idx as u32;
    let mut dev = RfkillDevice::empty();
    dev.id = id;
    dev.rfkill_type = rfkill_type;
    copy_name(&mut dev.name, name);
    dev.soft_blocked = false;
    dev.hard_blocked = false;
    dev.active = true;
    devices[slot_idx] = dev;
    Some(id)
}

/// Unregister (remove) a previously registered rfkill device.
///
/// Returns `true` on success, `false` if `id` is invalid or already inactive.
pub fn rfkill_unregister(id: u32) -> bool {
    if id as usize >= MAX_RFKILL {
        return false;
    }
    let mut devices = RFKILL_DEVICES.lock();
    if !devices[id as usize].active {
        return false;
    }
    devices[id as usize] = RfkillDevice::empty();
    true
}

/// Set (or clear) the software block for a device.
///
/// The soft block is only applied when the device is not hard-blocked;
/// attempting to change the soft state of a hard-blocked device returns
/// `false`.
///
/// Returns `true` on success, `false` if `id` is invalid, inactive, or
/// hard-blocked.
pub fn rfkill_set_soft_block(id: u32, block: bool) -> bool {
    if id as usize >= MAX_RFKILL {
        return false;
    }
    let mut devices = RFKILL_DEVICES.lock();
    let slot = &mut devices[id as usize];
    if !slot.active {
        return false;
    }
    // Cannot change soft state while hardware has overridden it
    if slot.hard_blocked {
        return false;
    }
    slot.soft_blocked = block;
    true
}

/// Set (or clear) the hardware block for a device.
///
/// The hard block always overrides the soft block and reflects physical
/// hardware state (e.g. a mechanical RF-kill switch or firmware line).
///
/// Returns `true` on success, `false` if `id` is invalid or inactive.
pub fn rfkill_set_hard_block(id: u32, block: bool) -> bool {
    if id as usize >= MAX_RFKILL {
        return false;
    }
    let mut devices = RFKILL_DEVICES.lock();
    let slot = &mut devices[id as usize];
    if !slot.active {
        return false;
    }
    slot.hard_blocked = block;
    true
}

/// Query the combined rfkill state of a device.
///
/// Returns `Some(RfkillState)` for a valid, active device, or `None` if `id`
/// is invalid or inactive.
pub fn rfkill_get_state(id: u32) -> Option<RfkillState> {
    if id as usize >= MAX_RFKILL {
        return None;
    }
    let devices = RFKILL_DEVICES.lock();
    let slot = &devices[id as usize];
    if !slot.active {
        return None;
    }
    Some(slot.state())
}

/// Soft-block all devices whose type matches `rfkill_type`.
///
/// If `rfkill_type` is `RFKILL_TYPE_ALL` (0), every active device is
/// soft-blocked regardless of its type.  Hard-blocked devices are skipped
/// (their state is controlled by hardware).
pub fn rfkill_block_all(rfkill_type: u8) {
    let mut devices = RFKILL_DEVICES.lock();
    for i in 0..MAX_RFKILL {
        if !devices[i].active {
            continue;
        }
        let type_match = rfkill_type == RFKILL_TYPE_ALL || devices[i].rfkill_type == rfkill_type;
        if type_match && !devices[i].hard_blocked {
            devices[i].soft_blocked = true;
        }
    }
}

/// Soft-unblock all devices whose type matches `rfkill_type`.
///
/// If `rfkill_type` is `RFKILL_TYPE_ALL` (0), every active device is
/// soft-unblocked.  Hard-blocked devices are skipped.
pub fn rfkill_unblock_all(rfkill_type: u8) {
    let mut devices = RFKILL_DEVICES.lock();
    for i in 0..MAX_RFKILL {
        if !devices[i].active {
            continue;
        }
        let type_match = rfkill_type == RFKILL_TYPE_ALL || devices[i].rfkill_type == rfkill_type;
        if type_match && !devices[i].hard_blocked {
            devices[i].soft_blocked = false;
        }
    }
}

/// Return `true` if the device is blocked by either soft or hard block.
///
/// Returns `false` for an invalid or inactive `id`.
pub fn rfkill_is_blocked(id: u32) -> bool {
    if id as usize >= MAX_RFKILL {
        return false;
    }
    let devices = RFKILL_DEVICES.lock();
    let slot = &devices[id as usize];
    if !slot.active {
        return false;
    }
    slot.soft_blocked || slot.hard_blocked
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the rfkill framework.
///
/// Currently performs no device registration (devices are registered by their
/// respective hardware drivers).  Prints a boot message.
pub fn init() {
    serial_println!("[rfkill] framework initialized");
}

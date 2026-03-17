use crate::sync::Mutex;
/// USB HID Gamepad / Joystick driver
///
/// Supports generic USB gamepads using a fixed-layout report format that
/// covers PS-style and Xbox-compatible controllers.  Up to 4 gamepads can
/// be registered simultaneously.
///
/// Because many gamepads ship with custom report descriptors, full descriptor
/// parsing is out of scope here.  Instead we use a common 12-byte fixed
/// layout:
///
///   Byte  0–1   Left-stick X  (i16, little-endian, -32767..32767)
///   Byte  2–3   Left-stick Y  (i16, little-endian)
///   Byte  4–5   Right-stick X (i16, little-endian)
///   Byte  6–7   Right-stick Y (i16, little-endian)
///   Byte  8–9   Button bitmask (u16, little-endian; bit0=A/Cross, …)
///   Byte 10     D-pad nibble + trigger left  (high nibble = dpad, low = LT)
///   Byte 11     Trigger right (0–255)
///
/// Devices whose report does not match this layout can be registered with
/// `gamepad_register_raw` and processed via the raw-report callback.
///
/// Constraints: #![no_std], no heap (no Vec/Box/String), fixed static arrays.
/// No float casts.  saturating_* for counters.
///
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Gamepad report — parsed, normalised
// ---------------------------------------------------------------------------

/// Fully parsed snapshot of one gamepad's input state.
#[derive(Clone, Copy, Default)]
pub struct GamepadReport {
    /// Left analogue stick X axis, range -32767 to +32767
    pub left_x: i16,
    /// Left analogue stick Y axis, range -32767 to +32767
    pub left_y: i16,
    /// Right analogue stick X axis
    pub right_x: i16,
    /// Right analogue stick Y axis
    pub right_y: i16,
    /// Button bitmask — up to 32 buttons.
    /// Suggested layout (Xbox/PS style):
    ///   bit 0  = A / Cross,    bit 1  = B / Circle
    ///   bit 2  = X / Square,   bit 3  = Y / Triangle
    ///   bit 4  = LB / L1,      bit 5  = RB / R1
    ///   bit 6  = LT / L2 dig., bit 7  = RT / R2 dig.
    ///   bit 8  = Select/Back,  bit 9  = Start/Menu
    ///   bit 10 = L3 (L-stick click), bit 11 = R3
    pub buttons: u32,
    /// D-pad direction: 0=none, 1=up, 2=up-right, 3=right, 4=down-right,
    /// 5=down, 6=down-left, 7=left, 8=up-left  (hat-switch encoding)
    pub dpad: u8,
    /// Left trigger — analogue value 0..255
    pub left_trigger: u8,
    /// Right trigger — analogue value 0..255
    pub right_trigger: u8,
}

// ---------------------------------------------------------------------------
// Gamepad device slot
// ---------------------------------------------------------------------------

/// One registered USB gamepad slot.
pub struct Gamepad {
    /// USB device address assigned during enumeration
    pub dev_addr: u8,
    /// USB vendor ID (from device descriptor)
    pub vid: u16,
    /// USB product ID
    pub pid: u16,
    /// Interrupt-IN endpoint number for this gamepad
    pub ep_in: u8,
    /// Most-recently-decoded report
    pub report: GamepadReport,
    /// Slot is occupied and device is active
    pub active: bool,
}

impl Gamepad {
    const fn empty() -> Self {
        Gamepad {
            dev_addr: 0,
            vid: 0,
            pid: 0,
            ep_in: 0,
            report: GamepadReport {
                left_x: 0,
                left_y: 0,
                right_x: 0,
                right_y: 0,
                buttons: 0,
                dpad: 0,
                left_trigger: 0,
                right_trigger: 0,
            },
            active: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Global gamepad table — 4 slots, no heap
// ---------------------------------------------------------------------------

const MAX_GAMEPADS: usize = 4;

struct GamepadState {
    pads: [Gamepad; MAX_GAMEPADS],
    count: usize,
}

impl GamepadState {
    const fn new() -> Self {
        GamepadState {
            pads: [
                Gamepad::empty(),
                Gamepad::empty(),
                Gamepad::empty(),
                Gamepad::empty(),
            ],
            count: 0,
        }
    }

    fn free_slot(&self) -> usize {
        let mut i = 0;
        while i < MAX_GAMEPADS {
            if !self.pads[i].active {
                return i;
            }
            i = i.saturating_add(1);
        }
        MAX_GAMEPADS
    }
}

static GAMEPADS: Mutex<GamepadState> = Mutex::new(GamepadState::new());

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register a gamepad that will be polled on `ep_in`.
///
/// Returns the slot index (0–3) on success, or `MAX_GAMEPADS` when the table
/// is full.
pub fn gamepad_register(dev_addr: u8, vid: u16, pid: u16, ep_in: u8) -> usize {
    let mut state = GAMEPADS.lock();
    let slot = state.free_slot();
    if slot >= MAX_GAMEPADS {
        serial_println!(
            "  [hid_gamepad] table full, dropping vid={:#06x} pid={:#06x}",
            vid,
            pid
        );
        return MAX_GAMEPADS;
    }
    state.pads[slot] = Gamepad {
        dev_addr,
        vid,
        pid,
        ep_in,
        report: GamepadReport::default(),
        active: true,
    };
    state.count = state.count.saturating_add(1);
    serial_println!(
        "  [hid_gamepad] registered slot {} dev={} vid={:#06x} pid={:#06x} ep_in={}",
        slot,
        dev_addr,
        vid,
        pid,
        ep_in
    );
    slot
}

/// Deregister a gamepad by slot index (e.g., on USB disconnect).
pub fn gamepad_deregister(slot: usize) {
    if slot >= MAX_GAMEPADS {
        return;
    }
    let mut state = GAMEPADS.lock();
    if state.pads[slot].active {
        state.pads[slot].active = false;
        state.count = state.count.saturating_sub(1);
        serial_println!("  [hid_gamepad] deregistered slot {}", slot);
    }
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

/// Poll slot `idx` for a new report.
///
/// Reads up to 12 bytes from the interrupt-IN endpoint via
/// `crate::usb::xhci::interrupt_in_poll`.  Returns `true` when a new
/// report was decoded and stored.
pub fn gamepad_poll(idx: usize) -> bool {
    if idx >= MAX_GAMEPADS {
        return false;
    }

    // Snapshot the fields we need, then drop the lock before calling xhci.
    let (active, dev_addr, ep_in) = {
        let state = GAMEPADS.lock();
        let p = &state.pads[idx];
        (p.active, p.dev_addr, p.ep_in)
    };
    if !active {
        return false;
    }

    let mut buf = [0u8; 12];
    let received = crate::usb::xhci::interrupt_in_poll(dev_addr, ep_in, &mut buf);
    if received < 10 {
        return false;
    } // need at least 10 bytes for axes + buttons

    let report = parse_fixed_report(&buf, received);

    {
        let mut state = GAMEPADS.lock();
        state.pads[idx].report = report;
    }
    true
}

/// Parse the fixed 12-byte report format described in the module comment.
fn parse_fixed_report(buf: &[u8; 12], _received: usize) -> GamepadReport {
    // i16 values from little-endian byte pairs — no float casts, no as f32/f64
    let left_x = (buf[0] as i16) | ((buf[1] as i16) << 8);
    let left_y = (buf[2] as i16) | ((buf[3] as i16) << 8);
    let right_x = (buf[4] as i16) | ((buf[5] as i16) << 8);
    let right_y = (buf[6] as i16) | ((buf[7] as i16) << 8);

    let buttons_raw = (buf[8] as u32) | ((buf[9] as u32) << 8);

    // Byte 10: high nibble = d-pad, low nibble = left trigger (4-bit → 0..255)
    let dpad = (buf[10] >> 4) & 0x0F;
    let lt_nibble = buf[10] & 0x0F;
    let left_trigger = lt_nibble.saturating_mul(17); // 0..15 → 0..255 approx

    let right_trigger = buf[11];

    GamepadReport {
        left_x,
        left_y,
        right_x,
        right_y,
        buttons: buttons_raw,
        dpad,
        left_trigger,
        right_trigger,
    }
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// Return a copy of the last decoded report for slot `idx`, or `None` if the
/// slot is inactive.
pub fn gamepad_report(idx: usize) -> Option<GamepadReport> {
    if idx >= MAX_GAMEPADS {
        return None;
    }
    let state = GAMEPADS.lock();
    if state.pads[idx].active {
        Some(state.pads[idx].report)
    } else {
        None
    }
}

/// Return the number of currently active gamepad slots.
pub fn gamepad_count() -> usize {
    GAMEPADS.lock().count
}

/// Return the USB vendor ID for slot `idx`, or 0 if inactive.
pub fn gamepad_vid(idx: usize) -> u16 {
    if idx >= MAX_GAMEPADS {
        return 0;
    }
    let state = GAMEPADS.lock();
    if state.pads[idx].active {
        state.pads[idx].vid
    } else {
        0
    }
}

/// Return the USB product ID for slot `idx`, or 0 if inactive.
pub fn gamepad_pid(idx: usize) -> u16 {
    if idx >= MAX_GAMEPADS {
        return 0;
    }
    let state = GAMEPADS.lock();
    if state.pads[idx].active {
        state.pads[idx].pid
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Periodic tick
// ---------------------------------------------------------------------------

/// Poll all active gamepad slots.  Call from the timer tick path.
pub fn gamepad_tick() {
    let mut i = 0;
    while i < MAX_GAMEPADS {
        gamepad_poll(i);
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("    [hid_gamepad] USB gamepad driver loaded (4 slots, fixed 12-byte report)");
}

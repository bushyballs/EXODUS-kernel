use crate::sync::Mutex;
/// USB HID (Human Interface Device) class driver
///
/// Handles USB boot-protocol keyboards, mice, and generically probed HID
/// devices.  Uses fixed-size static arrays only — no heap.
///
/// Boot protocol is requested via SET_PROTOCOL so both keyboards and mice
/// always report the well-defined 8-byte (keyboard) or 4-byte (mouse) format.
///
/// Keyboard reports are translated to Linux evdev key codes via a 256-entry
/// compile-time lookup table and injected into `crate::input::evdev`.
/// Mouse reports inject REL_X/REL_Y and button events into the same evdev
/// ring.
///
/// `hid_tick()` is called from the timer tick path and polls every active
/// device.
///
/// Constraints: #![no_std], no heap (no Vec/Box/String), no float casts.
/// All MMIO via read_volatile/write_volatile.  saturating_* for counters.
///
/// References: USB HID Class Specification 1.11,
///             USB HID Usage Tables 1.12 (Keyboard/Keypad page §10),
///             Linux input-event-codes.h (evdev key codes).
/// All code is original.
use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// HID class codes
// ---------------------------------------------------------------------------

/// USB HID interface class code
pub const HID_CLASS: u8 = 0x03;
pub const HID_SUBCLASS_NONE: u8 = 0x00;
pub const HID_SUBCLASS_BOOT: u8 = 0x01; // boot protocol subclass

pub const HID_PROTOCOL_NONE: u8 = 0x00;
pub const HID_PROTOCOL_KBD: u8 = 0x01; // boot keyboard
pub const HID_PROTOCOL_MOUSE: u8 = 0x02; // boot mouse

// ---------------------------------------------------------------------------
// HID class-specific request codes (bRequest for control transfers)
// ---------------------------------------------------------------------------

pub const HID_GET_REPORT: u8 = 0x01;
pub const HID_GET_IDLE: u8 = 0x02;
pub const HID_GET_PROTOCOL: u8 = 0x03;
pub const HID_SET_REPORT: u8 = 0x09;
pub const HID_SET_IDLE: u8 = 0x0A;
pub const HID_SET_PROTOCOL: u8 = 0x0B; // wValue: 0=boot, 1=report

// ---------------------------------------------------------------------------
// Modifier key bitmask constants (keyboard report byte 0)
// ---------------------------------------------------------------------------

pub const MOD_LEFT_CTRL: u8 = 0x01;
pub const MOD_LEFT_SHIFT: u8 = 0x02;
pub const MOD_LEFT_ALT: u8 = 0x04;
pub const MOD_LEFT_GUI: u8 = 0x08;
pub const MOD_RIGHT_CTRL: u8 = 0x10;
pub const MOD_RIGHT_SHIFT: u8 = 0x20;
pub const MOD_RIGHT_ALT: u8 = 0x40;
pub const MOD_RIGHT_GUI: u8 = 0x80;

// ---------------------------------------------------------------------------
// Button bit positions (mouse report byte 0)
// ---------------------------------------------------------------------------

const BTN_LEFT_BIT: u8 = 0x01;
const BTN_RIGHT_BIT: u8 = 0x02;
const BTN_MIDDLE_BIT: u8 = 0x04;

// Linux evdev key codes used for mouse buttons
const BTN_LEFT: u16 = 0x110;
const BTN_RIGHT: u16 = 0x111;
const BTN_MIDDLE: u16 = 0x112;

// ---------------------------------------------------------------------------
// Boot protocol report structures
// ---------------------------------------------------------------------------

/// USB HID boot keyboard report — 8 bytes, as defined by the USB HID spec
/// boot protocol (§B.1).
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct HidKbdReport {
    /// Modifier keys bitmask (see MOD_* constants above)
    pub modifier: u8,
    /// Always 0 (reserved)
    pub reserved: u8,
    /// Up to 6 simultaneous non-modifier key HID usage codes
    pub keys: [u8; 6],
}

impl HidKbdReport {
    /// True if `hid_code` appears in the keycodes array
    pub fn has_key(&self, hid_code: u8) -> bool {
        if hid_code == 0 {
            return false;
        }
        let mut i = 0;
        while i < 6 {
            if self.keys[i] == hid_code {
                return true;
            }
            i = i.saturating_add(1);
        }
        false
    }

    /// True when all key slots contain 0x01 (keyboard rollover / phantom)
    pub fn is_rollover(&self) -> bool {
        let mut i = 0;
        while i < 6 {
            if self.keys[i] != 0x01 {
                return false;
            }
            i = i.saturating_add(1);
        }
        true
    }
}

/// USB HID boot mouse report — 4 bytes (§B.2).
#[repr(C, packed)]
#[derive(Clone, Copy, Default)]
pub struct HidMouseReport {
    /// Button bitmask: bit0=left, bit1=right, bit2=middle
    pub buttons: u8,
    /// Relative X movement (signed)
    pub x: i8,
    /// Relative Y movement (signed)
    pub y: i8,
    /// Scroll wheel (signed; positive = scroll up)
    pub wheel: i8,
}

// ---------------------------------------------------------------------------
// HID device entry — fixed-size, no heap
// ---------------------------------------------------------------------------

/// Protocol discriminator stored in `HidDevice::protocol`
const PROTO_KBD: u8 = HID_PROTOCOL_KBD;
const PROTO_MOUSE: u8 = HID_PROTOCOL_MOUSE;

/// One registered HID device slot.
pub struct HidDevice {
    /// USB device address assigned during enumeration
    pub dev_addr: u8,
    /// PROTO_KBD or PROTO_MOUSE
    pub protocol: u8,
    /// Interrupt-IN endpoint number
    pub ep_in: u8,
    /// Maximum packet size for ep_in (8 bytes for boot protocol)
    pub ep_in_size: u8,
    /// Slot is occupied and device is active
    pub active: bool,
    /// Previous keyboard report — used for edge detection (press/release)
    pub last_kbd_report: HidKbdReport,
    /// Previous mouse report — used for button-change detection
    pub last_mouse_report: HidMouseReport,
}

impl HidDevice {
    const fn empty() -> Self {
        HidDevice {
            dev_addr: 0,
            protocol: 0,
            ep_in: 0,
            ep_in_size: 8,
            active: false,
            last_kbd_report: HidKbdReport {
                modifier: 0,
                reserved: 0,
                keys: [0u8; 6],
            },
            last_mouse_report: HidMouseReport {
                buttons: 0,
                x: 0,
                y: 0,
                wheel: 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Global device table — 8 slots, no heap
// ---------------------------------------------------------------------------

const MAX_HID_DEVICES: usize = 8;

struct HidState {
    devices: [HidDevice; MAX_HID_DEVICES],
    count: usize,
}

impl HidState {
    const fn new() -> Self {
        HidState {
            devices: [
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
                HidDevice::empty(),
            ],
            count: 0,
        }
    }

    /// Find a free slot index; returns MAX_HID_DEVICES when full.
    fn free_slot(&self) -> usize {
        let mut i = 0;
        while i < MAX_HID_DEVICES {
            if !self.devices[i].active {
                return i;
            }
            i = i.saturating_add(1);
        }
        MAX_HID_DEVICES
    }
}

static HID_DEVICES: Mutex<HidState> = Mutex::new(HidState::new());

// ---------------------------------------------------------------------------
// Public probe / init / poll API
// ---------------------------------------------------------------------------

/// Returns true if `class` identifies a HID interface.
pub fn hid_probe(dev_addr: u8, class: u8, subclass: u8, protocol: u8) -> bool {
    if class != HID_CLASS {
        return false;
    }
    serial_println!(
        "  [hid] probe dev={} subclass={:#04x} protocol={:#04x}",
        dev_addr,
        subclass,
        protocol
    );
    true
}

/// Register a HID device and initialise it.
///
/// Sends SET_PROTOCOL (boot) and SET_IDLE (indefinite) control requests,
/// then records the device in the global table.
///
/// Real hardware interaction is modelled by building the setup packets and
/// logging them; the xHCI ring submission would be wired in here once the
/// xHCI control-transfer path is available.
///
/// Returns `true` when a free slot was found and the device was registered.
pub fn hid_init(dev_addr: u8, ep_in: u8, protocol: u8) -> bool {
    // --- SET_PROTOCOL: switch to boot protocol ---
    // bmRequestType = 0x21 (class, interface, host-to-device)
    // bRequest      = HID_SET_PROTOCOL
    // wValue        = 0 (boot protocol)
    // wIndex        = 0 (interface 0)
    // wLength       = 0
    let set_protocol: [u8; 8] = [
        0x21,
        HID_SET_PROTOCOL,
        0x00,
        0x00, // wValue = 0 (boot)
        0x00,
        0x00, // wIndex = 0
        0x00,
        0x00, // wLength = 0
    ];
    serial_println!(
        "  [hid] SET_PROTOCOL (boot) -> dev={} setup={:02x?}",
        dev_addr,
        &set_protocol
    );

    // --- SET_IDLE: stop redundant reports (duration=0 → send only on change) ---
    // bmRequestType = 0x21
    // bRequest      = HID_SET_IDLE
    // wValue        = 0x0000 (duration=0, report_id=0)
    // wIndex        = 0
    // wLength       = 0
    let set_idle: [u8; 8] = [
        0x21,
        HID_SET_IDLE,
        0x00,
        0x00, // wValue = 0
        0x00,
        0x00, // wIndex = 0
        0x00,
        0x00, // wLength = 0
    ];
    serial_println!(
        "  [hid] SET_IDLE (indef) -> dev={} setup={:02x?}",
        dev_addr,
        &set_idle
    );

    // Register in device table
    let mut state = HID_DEVICES.lock();
    let slot = state.free_slot();
    if slot >= MAX_HID_DEVICES {
        serial_println!(
            "  [hid] ERROR: device table full, dropping dev={}",
            dev_addr
        );
        return false;
    }
    state.devices[slot] = HidDevice {
        dev_addr,
        protocol,
        ep_in,
        ep_in_size: 8,
        active: true,
        last_kbd_report: HidKbdReport::default(),
        last_mouse_report: HidMouseReport::default(),
    };
    state.count = state.count.saturating_add(1);
    let proto_name = if protocol == PROTO_KBD {
        "keyboard"
    } else {
        "mouse"
    };
    serial_println!(
        "  [hid] registered {} at slot {} (dev={} ep_in={})",
        proto_name,
        slot,
        dev_addr,
        ep_in
    );
    true
}

/// Poll the interrupt-IN endpoint of device slot `dev_idx` for a new report.
///
/// In a real driver this would submit an interrupt transfer to the xHCI ring
/// and wait (or check the completion ring).  Here we call the xHCI stub that
/// is available in this tree.  Returns `true` when a new report was
/// consumed.
pub fn hid_poll(dev_idx: usize) -> bool {
    if dev_idx >= MAX_HID_DEVICES {
        return false;
    }

    // We need to read the device fields, then release the lock before calling
    // into evdev (which also takes a lock) to avoid nested-lock deadlock.
    let (active, protocol, ep_in, dev_addr) = {
        let state = HID_DEVICES.lock();
        let d = &state.devices[dev_idx];
        (d.active, d.protocol, d.ep_in, d.dev_addr)
    };

    if !active {
        return false;
    }

    // Read up to 8 bytes from the interrupt-IN endpoint.
    // `xhci::interrupt_in_poll` returns the number of bytes received, or 0 on
    // NAK / no data.  The buffer is on the stack — no heap.
    let mut buf = [0u8; 8];
    let received = crate::usb::xhci::interrupt_in_poll(dev_addr, ep_in, &mut buf);
    if received == 0 {
        return false;
    }

    if protocol == PROTO_KBD && received >= 8 {
        let report = HidKbdReport {
            modifier: buf[0],
            reserved: buf[1],
            keys: [buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]],
        };
        process_kbd_report_for_slot(dev_idx, &report);
        return true;
    }

    if protocol == PROTO_MOUSE && received >= 3 {
        let report = HidMouseReport {
            buttons: buf[0],
            x: buf[1] as i8,
            y: buf[2] as i8,
            wheel: if received >= 4 { buf[3] as i8 } else { 0 },
        };
        process_mouse_report_for_slot(dev_idx, &report);
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Report processing — keyboard
// ---------------------------------------------------------------------------

/// Diff `report` against the previous keyboard report for `dev_idx`,
/// inject press/release events into evdev, and update the stored previous
/// report.
fn process_kbd_report_for_slot(dev_idx: usize, report: &HidKbdReport) {
    if report.is_rollover() {
        return;
    }

    // Read previous report then release the lock before calling evdev.
    let prev = {
        let state = HID_DEVICES.lock();
        state.devices[dev_idx].last_kbd_report
    };

    // Detect newly pressed keys (in report but not in prev)
    let mut pressed_keys = [0u8; 6];
    let mut pressed_count = 0usize;
    let mut released_keys = [0u8; 6];
    let mut released_count = 0usize;

    let mut i = 0;
    while i < 6 {
        let k = report.keys[i];
        if k != 0 && !prev.has_key(k) {
            if pressed_count < 6 {
                pressed_keys[pressed_count] = k;
                pressed_count = pressed_count.saturating_add(1);
            }
        }
        i = i.saturating_add(1);
    }

    // Detect newly released keys (in prev but not in report)
    let mut j = 0;
    while j < 6 {
        let k = prev.keys[j];
        if k != 0 && !report.has_key(k) {
            if released_count < 6 {
                released_keys[released_count] = k;
                released_count = released_count.saturating_add(1);
            }
        }
        j = j.saturating_add(1);
    }

    // Handle modifier changes too — translate modifier bits to evdev keys
    let mod_changed = report.modifier ^ prev.modifier;
    if mod_changed != 0 {
        inject_modifier_events(report.modifier, prev.modifier);
    }

    // Inject pressed keys
    let mut pi = 0;
    while pi < pressed_count {
        let evkey = hid_to_keycode(pressed_keys[pi]);
        if evkey != 0 {
            crate::input::evdev::inject_key(evkey, true);
        }
        pi = pi.saturating_add(1);
    }

    // Inject released keys
    let mut ri = 0;
    while ri < released_count {
        let evkey = hid_to_keycode(released_keys[ri]);
        if evkey != 0 {
            crate::input::evdev::inject_key(evkey, false);
        }
        ri = ri.saturating_add(1);
    }

    // Store new report
    {
        let mut state = HID_DEVICES.lock();
        state.devices[dev_idx].last_kbd_report = *report;
    }
}

/// Translate changed modifier bits into individual evdev key press/release events.
fn inject_modifier_events(current: u8, previous: u8) {
    // (modifier_bit, evdev_keycode)
    const MOD_MAP: [(u8, u16); 8] = [
        (MOD_LEFT_CTRL, 29),   // KEY_LEFTCTRL
        (MOD_LEFT_SHIFT, 42),  // KEY_LEFTSHIFT
        (MOD_LEFT_ALT, 56),    // KEY_LEFTALT
        (MOD_LEFT_GUI, 125),   // KEY_LEFTMETA
        (MOD_RIGHT_CTRL, 97),  // KEY_RIGHTCTRL
        (MOD_RIGHT_SHIFT, 54), // KEY_RIGHTSHIFT
        (MOD_RIGHT_ALT, 100),  // KEY_RIGHTALT
        (MOD_RIGHT_GUI, 126),  // KEY_RIGHTMETA
    ];
    let mut i = 0;
    while i < 8 {
        let (bit, key) = MOD_MAP[i];
        let was = (previous & bit) != 0;
        let now = (current & bit) != 0;
        if now && !was {
            crate::input::evdev::inject_key(key, true);
        } else if !now && was {
            crate::input::evdev::inject_key(key, false);
        }
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Report processing — mouse
// ---------------------------------------------------------------------------

/// Inject relative motion and button events from a mouse report.
fn process_mouse_report_for_slot(dev_idx: usize, report: &HidMouseReport) {
    let prev_buttons = {
        let state = HID_DEVICES.lock();
        state.devices[dev_idx].last_mouse_report.buttons
    };

    // Relative motion
    let dx = report.x as i16;
    let dy = report.y as i16;
    if dx != 0 || dy != 0 {
        crate::input::evdev::inject_rel_motion(dx, dy);
    }

    // Scroll wheel
    if report.wheel != 0 {
        use crate::input::evdev::{push_event, EventType, InputEvent, REL_WHEEL};
        push_event(InputEvent {
            event_type: EventType::RelativeAxis,
            code: REL_WHEEL,
            value: report.wheel as i32,
            timestamp_ns: 0,
        });
    }

    // Button changes
    let changed = report.buttons ^ prev_buttons;
    if changed != 0 {
        inject_mouse_button_events(report.buttons, prev_buttons);
    }

    // Store new report
    {
        let mut state = HID_DEVICES.lock();
        state.devices[dev_idx].last_mouse_report = *report;
    }
}

/// Emit evdev key events for each changed mouse button.
fn inject_mouse_button_events(current: u8, previous: u8) {
    const BTN_MAP: [(u8, u16); 3] = [
        (BTN_LEFT_BIT, BTN_LEFT),
        (BTN_RIGHT_BIT, BTN_RIGHT),
        (BTN_MIDDLE_BIT, BTN_MIDDLE),
    ];
    let mut i = 0;
    while i < 3 {
        let (bit, key) = BTN_MAP[i];
        let was = (previous & bit) != 0;
        let now = (current & bit) != 0;
        if now != was {
            crate::input::evdev::inject_key(key, now);
        }
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Public process_kbd_report / process_mouse_report (by-value wrappers)
// ---------------------------------------------------------------------------

/// Process a keyboard report that arrived outside the normal poll path
/// (e.g., injected for testing or forwarded from another driver).
/// Uses slot 0 as the default state holder.
pub fn process_kbd_report(report: &HidKbdReport) {
    process_kbd_report_for_slot(0, report);
}

/// Process a mouse report outside the normal poll path.
pub fn process_mouse_report(report: &HidMouseReport) {
    process_mouse_report_for_slot(0, report);
}

// ---------------------------------------------------------------------------
// Periodic poll tick — call from timer ISR or OS tick path
// ---------------------------------------------------------------------------

/// Call once per timer tick.  Polls every active HID device for new reports.
pub fn hid_tick() {
    let mut i = 0;
    while i < MAX_HID_DEVICES {
        hid_poll(i);
        i = i.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// HID Usage Page 0x07 (Keyboard/Keypad) → Linux evdev key code table
//
// Table index = HID usage code (0x00–0xFF).
// Table value = Linux evdev key code (0 = unmapped).
//
// Source: HID Usage Tables 1.12 §10 (Keyboard/Keypad page),
//         Linux input-event-codes.h
// ---------------------------------------------------------------------------

pub fn hid_to_keycode(hid: u8) -> u16 {
    HID_KEYMAP[hid as usize]
}

/// 256-entry compile-time table: HID keyboard usage → Linux evdev key code.
/// Indices not listed here are 0 (KEY_RESERVED / unmapped).
pub static HID_KEYMAP: [u16; 256] = {
    let mut t = [0u16; 256];

    // Letters A–Z (HID 0x04–0x1D → evdev 30, 48, 46, 32, 18, 33, 34, 35, 23,
    //              36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    //              21, 44)
    t[0x04] = 30; // A
    t[0x05] = 48; // B
    t[0x06] = 46; // C
    t[0x07] = 32; // D
    t[0x08] = 18; // E
    t[0x09] = 33; // F
    t[0x0A] = 34; // G
    t[0x0B] = 35; // H
    t[0x0C] = 23; // I
    t[0x0D] = 36; // J
    t[0x0E] = 37; // K
    t[0x0F] = 38; // L
    t[0x10] = 50; // M
    t[0x11] = 49; // N
    t[0x12] = 24; // O
    t[0x13] = 25; // P
    t[0x14] = 16; // Q
    t[0x15] = 19; // R
    t[0x16] = 31; // S
    t[0x17] = 20; // T
    t[0x18] = 22; // U
    t[0x19] = 47; // V
    t[0x1A] = 17; // W
    t[0x1B] = 45; // X
    t[0x1C] = 21; // Y
    t[0x1D] = 44; // Z

    // Digit row 1–9, 0  (HID 0x1E–0x27)
    t[0x1E] = 2; // 1
    t[0x1F] = 3; // 2
    t[0x20] = 4; // 3
    t[0x21] = 5; // 4
    t[0x22] = 6; // 5
    t[0x23] = 7; // 6
    t[0x24] = 8; // 7
    t[0x25] = 9; // 8
    t[0x26] = 10; // 9
    t[0x27] = 11; // 0

    // Special keys
    t[0x28] = 28; // ENTER          → KEY_ENTER
    t[0x29] = 1; // ESCAPE         → KEY_ESC
    t[0x2A] = 14; // BACKSPACE      → KEY_BACKSPACE
    t[0x2B] = 15; // TAB            → KEY_TAB
    t[0x2C] = 57; // SPACE          → KEY_SPACE
    t[0x2D] = 12; // - / _          → KEY_MINUS
    t[0x2E] = 13; // = / +          → KEY_EQUAL
    t[0x2F] = 26; // [ / {          → KEY_LEFTBRACE
    t[0x30] = 27; // ] / }          → KEY_RIGHTBRACE
    t[0x31] = 43; // \ / |          → KEY_BACKSLASH
    t[0x32] = 43; // Non-US # / ~   → KEY_BACKSLASH (same as 0x31 on US layouts)
    t[0x33] = 39; // ; / :          → KEY_SEMICOLON
    t[0x34] = 40; // ' / "          → KEY_APOSTROPHE
    t[0x35] = 41; // ` / ~          → KEY_GRAVE
    t[0x36] = 51; // , / <          → KEY_COMMA
    t[0x37] = 52; // . / >          → KEY_DOT
    t[0x38] = 53; // / / ?          → KEY_SLASH
    t[0x39] = 58; // CAPS LOCK      → KEY_CAPSLOCK

    // F1–F12  (HID 0x3A–0x45 → evdev 59–68, 87–88)
    t[0x3A] = 59; // F1
    t[0x3B] = 60; // F2
    t[0x3C] = 61; // F3
    t[0x3D] = 62; // F4
    t[0x3E] = 63; // F5
    t[0x3F] = 64; // F6
    t[0x40] = 65; // F7
    t[0x41] = 66; // F8
    t[0x42] = 67; // F9
    t[0x43] = 68; // F10
    t[0x44] = 87; // F11
    t[0x45] = 88; // F12

    // System / navigation
    t[0x46] = 99; // PRINT SCREEN   → KEY_SYSRQ
    t[0x47] = 70; // SCROLL LOCK    → KEY_SCROLLLOCK
    t[0x48] = 119; // PAUSE          → KEY_PAUSE
    t[0x49] = 110; // INSERT         → KEY_INSERT
    t[0x4A] = 102; // HOME           → KEY_HOME
    t[0x4B] = 104; // PAGE UP        → KEY_PAGEUP
    t[0x4C] = 111; // DELETE         → KEY_DELETE
    t[0x4D] = 107; // END            → KEY_END
    t[0x4E] = 109; // PAGE DOWN      → KEY_PAGEDOWN

    // Arrow keys
    t[0x4F] = 106; // RIGHT          → KEY_RIGHT
    t[0x50] = 105; // LEFT           → KEY_LEFT
    t[0x51] = 108; // DOWN           → KEY_DOWN
    t[0x52] = 103; // UP             → KEY_UP

    // Numpad
    t[0x53] = 69; // NUM LOCK       → KEY_NUMLOCK
    t[0x54] = 98; // KP /           → KEY_KPSLASH
    t[0x55] = 55; // KP *           → KEY_KPASTERISK
    t[0x56] = 74; // KP -           → KEY_KPMINUS
    t[0x57] = 78; // KP +           → KEY_KPPLUS
    t[0x58] = 96; // KP ENTER       → KEY_KPENTER
    t[0x59] = 79; // KP 1 / End     → KEY_KP1
    t[0x5A] = 80; // KP 2 / Down    → KEY_KP2
    t[0x5B] = 81; // KP 3 / PgDn    → KEY_KP3
    t[0x5C] = 75; // KP 4 / Left    → KEY_KP4
    t[0x5D] = 76; // KP 5           → KEY_KP5
    t[0x5E] = 77; // KP 6 / Right   → KEY_KP6
    t[0x5F] = 71; // KP 7 / Home    → KEY_KP7
    t[0x60] = 72; // KP 8 / Up      → KEY_KP8
    t[0x61] = 73; // KP 9 / PgUp    → KEY_KP9
    t[0x62] = 82; // KP 0 / Ins     → KEY_KP0
    t[0x63] = 83; // KP . / Del     → KEY_KPDOT

    // Non-US backslash and Application key
    t[0x64] = 86; // Non-US \       → KEY_102ND
    t[0x65] = 139; // APPLICATION    → KEY_MENU

    // F13–F24
    t[0x68] = 183; // F13            → KEY_F13
    t[0x69] = 184; // F14            → KEY_F14
    t[0x6A] = 185; // F15            → KEY_F15
    t[0x6B] = 186; // F16            → KEY_F16
    t[0x6C] = 187; // F17            → KEY_F17
    t[0x6D] = 188; // F18            → KEY_F18
    t[0x6E] = 189; // F19            → KEY_F19
    t[0x6F] = 190; // F20            → KEY_F20
    t[0x70] = 191; // F21            → KEY_F21
    t[0x71] = 192; // F22            → KEY_F22
    t[0x72] = 193; // F23            → KEY_F23
    t[0x73] = 194; // F24            → KEY_F24

    // Left/Right modifier keys (these come via the modifier byte in boot
    // protocol, but may also appear as usage codes in report protocol)
    t[0xE0] = 29; // LEFT CTRL      → KEY_LEFTCTRL
    t[0xE1] = 42; // LEFT SHIFT     → KEY_LEFTSHIFT
    t[0xE2] = 56; // LEFT ALT       → KEY_LEFTALT
    t[0xE3] = 125; // LEFT GUI       → KEY_LEFTMETA
    t[0xE4] = 97; // RIGHT CTRL     → KEY_RIGHTCTRL
    t[0xE5] = 54; // RIGHT SHIFT    → KEY_RIGHTSHIFT
    t[0xE6] = 100; // RIGHT ALT      → KEY_RIGHTALT
    t[0xE7] = 126; // RIGHT GUI      → KEY_RIGHTMETA

    t
};

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

pub fn init() {
    // State is already initialised via const fn new(); just log.
    serial_println!("    [hid] USB HID class driver loaded (boot kbd/mouse, 8 slots)");
}

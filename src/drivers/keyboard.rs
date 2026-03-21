use crate::io::{inb, io_wait, outb};
use crate::sync::Mutex;
/// PS/2 keyboard driver for Genesis
///
/// Full scan code set 1 translation, modifier tracking, LED control,
/// key repeat, ring buffer for IRQ-to-userspace delivery, and
/// special key combo handling (Ctrl+Alt+Del, SysRq, etc.).
///
/// The interrupt handler in interrupts.rs pushes scancodes here.
/// This module decodes them and manages the key buffer.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum key events in the ring buffer
const KEY_BUFFER_SIZE: usize = 256;

/// PS/2 data port
const PS2_DATA: u16 = 0x60;
/// PS/2 status/command port
const PS2_CMD: u16 = 0x64;

/// PS/2 keyboard commands
const KB_CMD_SET_LEDS: u8 = 0xED;
#[allow(dead_code)]
const KB_CMD_ECHO: u8 = 0xEE;
#[allow(dead_code)]
const KB_CMD_SCANCODE_SET: u8 = 0xF0;
#[allow(dead_code)]
const KB_CMD_IDENTIFY: u8 = 0xF2;
const KB_CMD_SET_TYPEMATIC: u8 = 0xF3;
const KB_CMD_ENABLE_SCANNING: u8 = 0xF4;
#[allow(dead_code)]
const KB_CMD_DISABLE_SCANNING: u8 = 0xF5;
const KB_CMD_SET_DEFAULTS: u8 = 0xF6;
const KB_CMD_RESET: u8 = 0xFF;

/// PS/2 keyboard responses
const KB_ACK: u8 = 0xFA;
const KB_RESEND: u8 = 0xFE;

/// Key repeat defaults (in ticks -- approximate at 1000 Hz timer)
const REPEAT_INITIAL_DELAY: u64 = 500; // ms before repeat starts
const REPEAT_RATE: u64 = 33; // ms between repeats (~30/sec)

/// Extended scancode prefix
const SCANCODE_EXTENDED: u8 = 0xE0;
/// Release flag in scan code set 1
const SCANCODE_RELEASE: u8 = 0x80;

// ---------------------------------------------------------------------------
// Key codes -- logical key identifiers independent of scan code set
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[allow(dead_code)]
pub enum KeyCode {
    Unknown = 0,
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    PrintScreen,
    ScrollLock,
    Pause,
    // Number row
    Backtick,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Key0,
    Minus,
    Equals,
    Backspace,
    // First alpha row
    Tab,
    Q,
    W,
    E,
    R,
    T,
    Y,
    U,
    I,
    O,
    P,
    LeftBracket,
    RightBracket,
    Backslash,
    // Second alpha row
    CapsLock,
    A,
    S,
    D,
    F,
    G,
    H,
    J,
    K,
    L,
    Semicolon,
    Apostrophe,
    Enter,
    // Third alpha row
    LeftShift,
    Z,
    X,
    C,
    V,
    B,
    N,
    M,
    Comma,
    Period,
    Slash,
    RightShift,
    // Bottom row
    LeftCtrl,
    LeftAlt,
    Space,
    RightAlt,
    RightCtrl,
    // Navigation
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    // Numpad
    NumLock,
    NumSlash,
    NumStar,
    NumMinus,
    Num7,
    Num8,
    Num9,
    NumPlus,
    Num4,
    Num5,
    Num6,
    Num1,
    Num2,
    Num3,
    NumEnter,
    Num0,
    NumDot,
    // Special
    SysRq,
    Menu,
    // Meta/Super/GUI keys
    LeftMeta,
    RightMeta,
}

// ---------------------------------------------------------------------------
// Structures
// ---------------------------------------------------------------------------

/// A key event
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct KeyEvent {
    /// The decoded character (0 if non-printable)
    pub character: char,
    /// Raw scancode from hardware
    pub scancode: u8,
    /// Logical key code
    pub keycode: KeyCode,
    /// Whether this is a key press (true) or release (false)
    pub pressed: bool,
    /// Modifier state at time of event
    pub modifiers: Modifiers,
}

/// Active modifier keys
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
}

/// Internal state for key repeat tracking
struct RepeatState {
    /// Scancode of the key currently held (0 = none)
    scancode: u8,
    /// Whether extended prefix was active
    extended: bool,
    /// Tick at which the key was first pressed
    press_tick: u64,
    /// Tick of last repeat event emitted
    last_repeat_tick: u64,
    /// Whether initial delay has passed
    repeating: bool,
}

/// Internal driver state (combined to reduce lock count)
struct KeyboardState {
    modifiers: Modifiers,
    extended_pending: bool,
    repeat: RepeatState,
    led_state: u8, // bits: 0=scroll, 1=num, 2=caps
    tick_counter: u64,
}

// ---------------------------------------------------------------------------
// Scan code set 1 translation tables
// ---------------------------------------------------------------------------

/// Normal (unshifted) character for scan code set 1 codes 0x00..0x58
static SCANCODE_TO_CHAR: [char; 89] = [
    '\0', '\x1B', '1', '2', '3', '4', '5', '6', // 0x00-0x07
    '7', '8', '9', '0', '-', '=', '\x08', '\t', // 0x08-0x0F
    'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', // 0x10-0x17
    'o', 'p', '[', ']', '\n', '\0', 'a', 's', // 0x18-0x1F
    'd', 'f', 'g', 'h', 'j', 'k', 'l', ';', // 0x20-0x27
    '\'', '`', '\0', '\\', 'z', 'x', 'c', 'v', // 0x28-0x2F
    'b', 'n', 'm', ',', '.', '/', '\0', '*', // 0x30-0x37
    '\0', ' ', '\0', '\0', '\0', '\0', '\0', '\0', // 0x38-0x3F (alt, space, caps, F1-F4)
    '\0', '\0', '\0', '\0', '\0', '\0', '\0',
    '7', // 0x40-0x47 (F5-F10, numlock, scrolllock, num7)
    '8', '9', '-', '4', '5', '6', '+', '1', // 0x48-0x4F
    '2', '3', '0', '.', '\0', '\0', '\0', '\0', // 0x50-0x57 (num2,num3,num0,numdot,..,F11)
    '\0', // 0x58 = F12
];

/// Shifted character for scan code set 1 codes 0x00..0x58
static SCANCODE_TO_CHAR_SHIFT: [char; 89] = [
    '\0', '\x1B', '!', '@', '#', '$', '%', '^', // 0x00-0x07
    '&', '*', '(', ')', '_', '+', '\x08', '\t', // 0x08-0x0F
    'Q', 'W', 'E', 'R', 'T', 'Y', 'U', 'I', // 0x10-0x17
    'O', 'P', '{', '}', '\n', '\0', 'A', 'S', // 0x18-0x1F
    'D', 'F', 'G', 'H', 'J', 'K', 'L', ':', // 0x20-0x27
    '"', '~', '\0', '|', 'Z', 'X', 'C', 'V', // 0x28-0x2F
    'B', 'N', 'M', '<', '>', '?', '\0', '*', // 0x30-0x37
    '\0', ' ', '\0', '\0', '\0', '\0', '\0', '\0', // 0x38-0x3F
    '\0', '\0', '\0', '\0', '\0', '\0', '\0', '7', // 0x40-0x47
    '8', '9', '-', '4', '5', '6', '+', '1', // 0x48-0x4F
    '2', '3', '0', '.', '\0', '\0', '\0', '\0', // 0x50-0x57
    '\0', // 0x58
];

/// Scan code set 1 -> KeyCode mapping (normal, non-extended)
static SCANCODE_TO_KEYCODE: [KeyCode; 89] = [
    KeyCode::Unknown,
    KeyCode::Escape,
    KeyCode::Key1,
    KeyCode::Key2, // 0x00-0x03
    KeyCode::Key3,
    KeyCode::Key4,
    KeyCode::Key5,
    KeyCode::Key6, // 0x04-0x07
    KeyCode::Key7,
    KeyCode::Key8,
    KeyCode::Key9,
    KeyCode::Key0, // 0x08-0x0B
    KeyCode::Minus,
    KeyCode::Equals,
    KeyCode::Backspace,
    KeyCode::Tab, // 0x0C-0x0F
    KeyCode::Q,
    KeyCode::W,
    KeyCode::E,
    KeyCode::R, // 0x10-0x13
    KeyCode::T,
    KeyCode::Y,
    KeyCode::U,
    KeyCode::I, // 0x14-0x17
    KeyCode::O,
    KeyCode::P,
    KeyCode::LeftBracket,
    KeyCode::RightBracket, // 0x18-0x1B
    KeyCode::Enter,
    KeyCode::LeftCtrl,
    KeyCode::A,
    KeyCode::S, // 0x1C-0x1F
    KeyCode::D,
    KeyCode::F,
    KeyCode::G,
    KeyCode::H, // 0x20-0x23
    KeyCode::J,
    KeyCode::K,
    KeyCode::L,
    KeyCode::Semicolon, // 0x24-0x27
    KeyCode::Apostrophe,
    KeyCode::Backtick,
    KeyCode::LeftShift,
    KeyCode::Backslash, // 0x28-0x2B
    KeyCode::Z,
    KeyCode::X,
    KeyCode::C,
    KeyCode::V, // 0x2C-0x2F
    KeyCode::B,
    KeyCode::N,
    KeyCode::M,
    KeyCode::Comma, // 0x30-0x33
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::RightShift,
    KeyCode::NumStar, // 0x34-0x37
    KeyCode::LeftAlt,
    KeyCode::Space,
    KeyCode::CapsLock,
    KeyCode::F1, // 0x38-0x3B
    KeyCode::F2,
    KeyCode::F3,
    KeyCode::F4,
    KeyCode::F5, // 0x3C-0x3F
    KeyCode::F6,
    KeyCode::F7,
    KeyCode::F8,
    KeyCode::F9, // 0x40-0x43
    KeyCode::F10,
    KeyCode::NumLock,
    KeyCode::ScrollLock,
    KeyCode::Num7, // 0x44-0x47
    KeyCode::Num8,
    KeyCode::Num9,
    KeyCode::NumMinus,
    KeyCode::Num4, // 0x48-0x4B
    KeyCode::Num5,
    KeyCode::Num6,
    KeyCode::NumPlus,
    KeyCode::Num1, // 0x4C-0x4F
    KeyCode::Num2,
    KeyCode::Num3,
    KeyCode::Num0,
    KeyCode::NumDot, // 0x50-0x53
    KeyCode::Unknown,
    KeyCode::Unknown,
    KeyCode::Unknown,
    KeyCode::F11, // 0x54-0x57
    KeyCode::F12, // 0x58
];

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Key buffer -- ring buffer of key events
static KEY_BUFFER: Mutex<VecDeque<KeyEvent>> = Mutex::new(VecDeque::new());

/// Internal keyboard state
static KB_STATE: Mutex<KeyboardState> = Mutex::new(KeyboardState {
    modifiers: Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        caps_lock: false,
        num_lock: false,
        scroll_lock: false,
    },
    extended_pending: false,
    repeat: RepeatState {
        scancode: 0,
        extended: false,
        press_tick: 0,
        last_repeat_tick: 0,
        repeating: false,
    },
    led_state: 0,
    tick_counter: 0,
});

// Preserve the old static for backward compatibility with any external code
// referencing MODIFIERS directly; we keep it in sync.
/// Current modifier state (legacy accessor)
static MODIFIERS: Mutex<Modifiers> = Mutex::new(Modifiers {
    shift: false,
    ctrl: false,
    alt: false,
    caps_lock: false,
    num_lock: false,
    scroll_lock: false,
});

// ---------------------------------------------------------------------------
// PS/2 controller helpers
// ---------------------------------------------------------------------------

/// Wait until the PS/2 input buffer is ready for a command byte
fn wait_input_ready() {
    for _ in 0..100_000 {
        if inb(PS2_CMD) & 0x02 == 0 {
            return;
        }
        io_wait();
    }
}

/// Wait until the PS/2 output buffer has data to read
fn wait_output_ready() {
    for _ in 0..100_000 {
        if inb(PS2_CMD) & 0x01 != 0 {
            return;
        }
        io_wait();
    }
}

/// Send a command byte to the keyboard and wait for ACK
fn kb_send_cmd(cmd: u8) -> bool {
    for _retry in 0..3 {
        wait_input_ready();
        outb(PS2_DATA, cmd);
        wait_output_ready();
        let resp = inb(PS2_DATA);
        if resp == KB_ACK {
            return true;
        }
        if resp == KB_RESEND {
            continue;
        }
    }
    false
}

/// Send a command byte followed by a data byte
fn kb_send_cmd_data(cmd: u8, data: u8) -> bool {
    if !kb_send_cmd(cmd) {
        return false;
    }
    wait_input_ready();
    outb(PS2_DATA, data);
    wait_output_ready();
    let resp = inb(PS2_DATA);
    resp == KB_ACK
}

// ---------------------------------------------------------------------------
// LED control
// ---------------------------------------------------------------------------

/// Update keyboard LEDs to reflect current lock states.
/// Bits: 0 = Scroll Lock, 1 = Num Lock, 2 = Caps Lock.
fn update_leds(state: &KeyboardState) {
    let led_bits: u8 = (if state.modifiers.scroll_lock { 1 } else { 0 })
        | (if state.modifiers.num_lock { 2 } else { 0 })
        | (if state.modifiers.caps_lock { 4 } else { 0 });
    // Only send if changed
    if led_bits != state.led_state {
        // We cannot mutate state here, but we can do the I/O;
        // caller updates led_state.
        let _ = kb_send_cmd_data(KB_CMD_SET_LEDS, led_bits);
    }
}

// ---------------------------------------------------------------------------
// Extended key code mapping (0xE0 prefix)
// ---------------------------------------------------------------------------

fn extended_scancode_to_keycode(code: u8) -> KeyCode {
    match code {
        0x1C => KeyCode::NumEnter,
        0x1D => KeyCode::RightCtrl,
        0x35 => KeyCode::NumSlash,
        0x37 => KeyCode::PrintScreen,
        0x38 => KeyCode::RightAlt,
        0x47 => KeyCode::Home,
        0x48 => KeyCode::ArrowUp,
        0x49 => KeyCode::PageUp,
        0x4B => KeyCode::ArrowLeft,
        0x4D => KeyCode::ArrowRight,
        0x4F => KeyCode::End,
        0x50 => KeyCode::ArrowDown,
        0x51 => KeyCode::PageDown,
        0x52 => KeyCode::Insert,
        0x53 => KeyCode::Delete,
        0x5B => KeyCode::Menu, // Left GUI / Super
        0x5C => KeyCode::Menu, // Right GUI / Super
        0x5D => KeyCode::Menu, // Apps/Menu key
        _ => KeyCode::Unknown,
    }
}

/// Map extended key codes to escape sequences for terminal applications
#[allow(dead_code)]
fn keycode_to_escape_seq(kc: KeyCode) -> Option<&'static [u8]> {
    match kc {
        KeyCode::ArrowUp => Some(b"\x1B[A"),
        KeyCode::ArrowDown => Some(b"\x1B[B"),
        KeyCode::ArrowRight => Some(b"\x1B[C"),
        KeyCode::ArrowLeft => Some(b"\x1B[D"),
        KeyCode::Home => Some(b"\x1B[H"),
        KeyCode::End => Some(b"\x1B[F"),
        KeyCode::Insert => Some(b"\x1B[2~"),
        KeyCode::Delete => Some(b"\x1B[3~"),
        KeyCode::PageUp => Some(b"\x1B[5~"),
        KeyCode::PageDown => Some(b"\x1B[6~"),
        KeyCode::F1 => Some(b"\x1BOP"),
        KeyCode::F2 => Some(b"\x1BOQ"),
        KeyCode::F3 => Some(b"\x1BOR"),
        KeyCode::F4 => Some(b"\x1BOS"),
        KeyCode::F5 => Some(b"\x1B[15~"),
        KeyCode::F6 => Some(b"\x1B[17~"),
        KeyCode::F7 => Some(b"\x1B[18~"),
        KeyCode::F8 => Some(b"\x1B[19~"),
        KeyCode::F9 => Some(b"\x1B[20~"),
        KeyCode::F10 => Some(b"\x1B[21~"),
        KeyCode::F11 => Some(b"\x1B[23~"),
        KeyCode::F12 => Some(b"\x1B[24~"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Scancode processing -- called from IRQ handler
// ---------------------------------------------------------------------------

/// Process a raw scancode byte from the PS/2 controller.
/// This is the primary entry point called by the IRQ1 handler.
pub fn process_scancode(raw: u8) {
    let mut state = KB_STATE.lock();

    // Handle extended prefix
    if raw == SCANCODE_EXTENDED {
        state.extended_pending = true;
        return;
    }

    let extended = state.extended_pending;
    state.extended_pending = false;

    let pressed = raw & SCANCODE_RELEASE == 0;
    let code = raw & 0x7F;

    // Determine keycode
    let keycode = if extended {
        extended_scancode_to_keycode(code)
    } else if (code as usize) < SCANCODE_TO_KEYCODE.len() {
        SCANCODE_TO_KEYCODE[code as usize]
    } else {
        KeyCode::Unknown
    };

    // Update modifier state
    match keycode {
        KeyCode::LeftShift | KeyCode::RightShift => state.modifiers.shift = pressed,
        KeyCode::LeftCtrl | KeyCode::RightCtrl => state.modifiers.ctrl = pressed,
        KeyCode::LeftAlt | KeyCode::RightAlt => state.modifiers.alt = pressed,
        KeyCode::CapsLock if pressed => {
            state.modifiers.caps_lock = !state.modifiers.caps_lock;
            update_leds(&state);
            let led_bits = (if state.modifiers.scroll_lock { 1u8 } else { 0 })
                | (if state.modifiers.num_lock { 2 } else { 0 })
                | (if state.modifiers.caps_lock { 4 } else { 0 });
            state.led_state = led_bits;
        }
        KeyCode::NumLock if pressed => {
            state.modifiers.num_lock = !state.modifiers.num_lock;
            update_leds(&state);
            let led_bits = (if state.modifiers.scroll_lock { 1u8 } else { 0 })
                | (if state.modifiers.num_lock { 2 } else { 0 })
                | (if state.modifiers.caps_lock { 4 } else { 0 });
            state.led_state = led_bits;
        }
        KeyCode::ScrollLock if pressed => {
            state.modifiers.scroll_lock = !state.modifiers.scroll_lock;
            update_leds(&state);
            let led_bits = (if state.modifiers.scroll_lock { 1u8 } else { 0 })
                | (if state.modifiers.num_lock { 2 } else { 0 })
                | (if state.modifiers.caps_lock { 4 } else { 0 });
            state.led_state = led_bits;
        }
        _ => {}
    }

    // Special key combos
    if pressed {
        // Ctrl+Alt+Del -> reboot
        if state.modifiers.ctrl && state.modifiers.alt && keycode == KeyCode::Delete {
            drop(state);
            serial_println!("  Keyboard: Ctrl+Alt+Del -> reboot");
            reboot();
            return;
        }

        // SysRq (Alt + PrintScreen)
        if state.modifiers.alt && keycode == KeyCode::PrintScreen {
            drop(state);
            serial_println!("  Keyboard: SysRq triggered");
            handle_sysrq();
            return;
        }
    }

    // Determine character value
    let character = if extended {
        // Extended keys generally don't produce characters
        '\0'
    } else if (code as usize) < SCANCODE_TO_CHAR.len() {
        let shift_active = state.modifiers.shift;
        let caps = state.modifiers.caps_lock;

        let base = SCANCODE_TO_CHAR[code as usize];
        let shifted = SCANCODE_TO_CHAR_SHIFT[code as usize];

        // Determine effective shift for letters (caps lock inverts shift)
        let is_letter = base.is_ascii_alphabetic();
        let effective_shift = if is_letter {
            shift_active ^ caps
        } else {
            shift_active
        };

        if effective_shift {
            shifted
        } else {
            base
        }
    } else {
        '\0'
    };

    // Handle Ctrl key character mapping (Ctrl+A = 0x01, Ctrl+C = 0x03, etc.)
    let character = if state.modifiers.ctrl && character.is_ascii_alphabetic() {
        let ctrl_char = (character.to_ascii_uppercase() as u8)
            .saturating_sub(b'A')
            .saturating_add(1);
        ctrl_char as char
    } else {
        character
    };

    let mods = state.modifiers;

    // Key repeat tracking
    if pressed {
        // Only track repeat for non-modifier keys
        match keycode {
            KeyCode::LeftShift
            | KeyCode::RightShift
            | KeyCode::LeftCtrl
            | KeyCode::RightCtrl
            | KeyCode::LeftAlt
            | KeyCode::RightAlt
            | KeyCode::CapsLock
            | KeyCode::NumLock
            | KeyCode::ScrollLock => {}
            _ => {
                state.repeat.scancode = code;
                state.repeat.extended = extended;
                state.repeat.press_tick = state.tick_counter;
                state.repeat.last_repeat_tick = state.tick_counter;
                state.repeat.repeating = false;
            }
        }
    } else {
        // Key released -- stop repeat if it's the same key
        if state.repeat.scancode == code && state.repeat.extended == extended {
            state.repeat.scancode = 0;
            state.repeat.repeating = false;
        }
    }

    // Sync legacy MODIFIERS static
    drop(state);
    *MODIFIERS.lock() = mods;

    // Build and enqueue the event
    let event = KeyEvent {
        character,
        scancode: raw,
        keycode,
        pressed,
        modifiers: mods,
    };

    push_key(event);
}

// ---------------------------------------------------------------------------
// Key repeat -- called periodically from the timer tick handler
// ---------------------------------------------------------------------------

/// Advance the tick counter and generate repeat events if a key is held.
/// Call this from the timer interrupt handler (every ~1 ms).
pub fn tick() {
    let mut state = KB_STATE.lock();
    state.tick_counter = state.tick_counter.saturating_add(1);

    if state.repeat.scancode == 0 {
        return;
    }

    let elapsed = state.tick_counter - state.repeat.press_tick;

    if !state.repeat.repeating {
        if elapsed >= REPEAT_INITIAL_DELAY {
            state.repeat.repeating = true;
            state.repeat.last_repeat_tick = state.tick_counter;
            // Emit repeat event
            let code = state.repeat.scancode;
            let extended = state.repeat.extended;
            let mods = state.modifiers;
            drop(state);
            emit_repeat(code, extended, mods);
        }
    } else {
        let since_last = state.tick_counter.saturating_sub(state.repeat.last_repeat_tick);
        if since_last >= REPEAT_RATE {
            state.repeat.last_repeat_tick = state.tick_counter;
            let code = state.repeat.scancode;
            let extended = state.repeat.extended;
            let mods = state.modifiers;
            drop(state);
            emit_repeat(code, extended, mods);
        }
    }
}

/// Emit a repeat event for the given scancode
fn emit_repeat(code: u8, extended: bool, mods: Modifiers) {
    let keycode = if extended {
        extended_scancode_to_keycode(code)
    } else if (code as usize) < SCANCODE_TO_KEYCODE.len() {
        SCANCODE_TO_KEYCODE[code as usize]
    } else {
        KeyCode::Unknown
    };

    let character = if extended {
        '\0'
    } else if (code as usize) < SCANCODE_TO_CHAR.len() {
        let is_letter = SCANCODE_TO_CHAR[code as usize].is_ascii_alphabetic();
        let effective_shift = if is_letter {
            mods.shift ^ mods.caps_lock
        } else {
            mods.shift
        };
        if effective_shift {
            SCANCODE_TO_CHAR_SHIFT[code as usize]
        } else {
            SCANCODE_TO_CHAR[code as usize]
        }
    } else {
        '\0'
    };

    let character = if mods.ctrl && character.is_ascii_alphabetic() {
        ((character.to_ascii_uppercase() as u8)
            .saturating_sub(b'A')
            .saturating_add(1)) as char
    } else {
        character
    };

    let event = KeyEvent {
        character,
        scancode: code,
        keycode,
        pressed: true,
        modifiers: mods,
    };
    push_key(event);
}

// ---------------------------------------------------------------------------
// Special actions
// ---------------------------------------------------------------------------

/// Reboot the machine via PS/2 controller pulse
fn reboot() {
    serial_println!("  Keyboard: initiating reboot via 8042 pulse");
    // Try the keyboard controller reset line
    wait_input_ready();
    outb(PS2_CMD, 0xFE); // pulse CPU reset line
                         // If that didn't work, triple-fault
    loop {
        crate::io::hlt();
    }
}

/// Handle SysRq key press (Alt+PrintScreen)
fn handle_sysrq() {
    // Placeholder for SysRq magic keys. In Linux this triggers
    // various emergency actions (sync, umount, reboot, etc.)
    serial_println!("  SysRq: emergency debug dump");
    // Dump basic system info
    serial_println!("  SysRq: key buffer size = {}", KEY_BUFFER.lock().len());
}

// ---------------------------------------------------------------------------
// Public API -- buffer management
// ---------------------------------------------------------------------------

/// Push a key event into the buffer (called from interrupt handler)
/// Also injects the event into the unified evdev ring so that consumers
/// using the evdev API see the same events.
pub fn push_key(event: KeyEvent) {
    let mut buf = KEY_BUFFER.lock();
    if buf.len() >= KEY_BUFFER_SIZE {
        buf.pop_front(); // drop oldest if full
    }
    buf.push_back(event);
    // Mirror into the unified evdev event queue.
    // Use the raw scancode as the evdev key code (scan code set 1).
    let key_code = event.keycode as u16;
    crate::input::evdev::inject_key(key_code, event.pressed);
}

/// Pop a key event from the buffer (called by userspace/shell)
pub fn pop_key() -> Option<KeyEvent> {
    KEY_BUFFER.lock().pop_front()
}

/// Check if there are key events available
#[allow(dead_code)]
pub fn has_key() -> bool {
    !KEY_BUFFER.lock().is_empty()
}

/// Get current modifier state
#[allow(dead_code)]
pub fn modifiers() -> Modifiers {
    *MODIFIERS.lock()
}

/// Read a line from the keyboard buffer (blocking would need scheduler)
/// For now, returns whatever is available
#[allow(dead_code)]
pub fn read_line() -> alloc::string::String {
    let mut line = alloc::string::String::new();
    let mut buf = KEY_BUFFER.lock();

    while let Some(event) = buf.pop_front() {
        if event.pressed && event.character != '\0' {
            if event.character == '\n' {
                break;
            }
            line.push(event.character);
        }
    }

    line
}

/// Read a single character, blocking-style (poll version).
/// Returns None if no key is available.
#[allow(dead_code)]
pub fn read_char() -> Option<char> {
    let mut buf = KEY_BUFFER.lock();
    loop {
        match buf.pop_front() {
            Some(event) if event.pressed && event.character != '\0' => {
                return Some(event.character);
            }
            Some(_) => continue, // skip releases and non-printable presses
            None => return None,
        }
    }
}

/// Get escape sequence bytes for a key event (for terminal emulation).
/// Returns None for keys that don't produce escape sequences.
#[allow(dead_code)]
pub fn get_escape_sequence(event: &KeyEvent) -> Option<&'static [u8]> {
    if !event.pressed {
        return None;
    }
    keycode_to_escape_seq(event.keycode)
}

/// Flush the key buffer
#[allow(dead_code)]
pub fn flush() {
    KEY_BUFFER.lock().clear();
}

/// Get the number of pending key events
#[allow(dead_code)]
pub fn pending_count() -> usize {
    KEY_BUFFER.lock().len()
}

// ---------------------------------------------------------------------------
// Typematic rate / delay configuration
// ---------------------------------------------------------------------------

/// Set the typematic rate and delay on the hardware keyboard.
/// `delay_code`: 0 = 250ms, 1 = 500ms, 2 = 750ms, 3 = 1000ms
/// `rate_code`: 0 = 30/sec, ... 0x1F = 2/sec (see PS/2 spec)
#[allow(dead_code)]
pub fn set_typematic(delay_code: u8, rate_code: u8) {
    let val = ((delay_code & 0x03) << 5) | (rate_code & 0x1F);
    let _ = kb_send_cmd_data(KB_CMD_SET_TYPEMATIC, val);
    serial_println!(
        "  Keyboard: typematic set delay={} rate={}",
        delay_code,
        rate_code
    );
}

// ---------------------------------------------------------------------------
// Special key codes (non-printable, passed via scancode field) -- legacy API
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub mod special {
    pub const ARROW_UP: u8 = 0x80;
    pub const ARROW_DOWN: u8 = 0x81;
    pub const ARROW_LEFT: u8 = 0x82;
    pub const ARROW_RIGHT: u8 = 0x83;
    pub const CTRL_C: u8 = 0x84;
    pub const CTRL_Z: u8 = 0x85;
    pub const CTRL_D: u8 = 0x86;
    pub const TAB: u8 = 0x87;
    pub const HOME: u8 = 0x88;
    pub const END: u8 = 0x89;
    pub const PAGE_UP: u8 = 0x8A;
    pub const PAGE_DOWN: u8 = 0x8B;
    pub const DELETE: u8 = 0x8C;
}

// ---------------------------------------------------------------------------
// Initialize the keyboard driver
// ---------------------------------------------------------------------------

pub fn init() {
    // Reset keyboard
    let _ = kb_send_cmd(KB_CMD_RESET);
    // Small delay after reset
    for _ in 0..10_000 {
        io_wait();
    }

    // Set defaults
    let _ = kb_send_cmd(KB_CMD_SET_DEFAULTS);

    // Enable scanning
    let _ = kb_send_cmd(KB_CMD_ENABLE_SCANNING);

    // Set default typematic: 500ms delay, 10.9 chars/sec
    let _ = kb_send_cmd_data(KB_CMD_SET_TYPEMATIC, 0x20);

    // Clear any pending data
    while inb(PS2_CMD) & 0x01 != 0 {
        let _ = inb(PS2_DATA);
    }

    // Turn off all LEDs initially
    let _ = kb_send_cmd_data(KB_CMD_SET_LEDS, 0x00);

    super::register("ps2-keyboard", super::DeviceType::Keyboard);
    serial_println!(
        "  Keyboard: PS/2 driver ready, buffer size {}",
        KEY_BUFFER_SIZE
    );
}

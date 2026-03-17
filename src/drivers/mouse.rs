use crate::io::{inb, io_wait, outb};
use crate::sync::Mutex;
/// PS/2 mouse driver for Genesis
///
/// Handles PS/2 mouse packets (3 or 4 bytes) for cursor movement,
/// button clicks, and scroll wheel events. Includes IntelliMouse
/// protocol detection, acceleration curves, double-click detection,
/// and a ring buffer event queue.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// PS/2 controller ports
const PS2_DATA: u16 = 0x60;
const PS2_CMD: u16 = 0x64;

/// Mouse event queue capacity
const MOUSE_QUEUE_SIZE: usize = 256;

/// Double-click threshold in ticks (ms at 1 kHz timer)
const DOUBLE_CLICK_THRESHOLD: u64 = 400;
/// Maximum pixel distance between clicks for double-click
const DOUBLE_CLICK_DISTANCE: i32 = 8;

/// Acceleration thresholds (integer-based curve)
/// If delta magnitude exceeds a threshold, multiply by a scale factor.
/// We use fixed-point with 8 fractional bits (scale 256 = 1.0x).
const ACCEL_THRESHOLD_1: i32 = 4; // pixels per packet
const ACCEL_SCALE_1: i32 = 384; // 1.5x (384/256)
const ACCEL_THRESHOLD_2: i32 = 8;
const ACCEL_SCALE_2: i32 = 512; // 2.0x
const ACCEL_THRESHOLD_3: i32 = 16;
const ACCEL_SCALE_3: i32 = 768; // 3.0x

/// PS/2 mouse command bytes
const MOUSE_CMD_RESET: u8 = 0xFF;
const MOUSE_CMD_RESEND: u8 = 0xFE;
const MOUSE_CMD_SET_DEFAULTS: u8 = 0xF6;
const MOUSE_CMD_DISABLE_REPORTING: u8 = 0xF5;
const MOUSE_CMD_ENABLE_REPORTING: u8 = 0xF4;
const MOUSE_CMD_SET_SAMPLE_RATE: u8 = 0xF3;
const MOUSE_CMD_GET_DEVICE_ID: u8 = 0xF2;
const MOUSE_CMD_SET_REMOTE_MODE: u8 = 0xF0;
const MOUSE_CMD_SET_WRAP_MODE: u8 = 0xEE;
const MOUSE_CMD_RESET_WRAP_MODE: u8 = 0xEC;
const MOUSE_CMD_READ_DATA: u8 = 0xEB;
const MOUSE_CMD_SET_STREAM_MODE: u8 = 0xEA;
const MOUSE_CMD_STATUS_REQUEST: u8 = 0xE9;
const MOUSE_CMD_SET_RESOLUTION: u8 = 0xE8;
const MOUSE_CMD_SET_SCALING_2_1: u8 = 0xE7;
const MOUSE_CMD_SET_SCALING_1_1: u8 = 0xE6;

/// Mouse button masks
pub const BUTTON_LEFT: u8 = 0x01;
pub const BUTTON_RIGHT: u8 = 0x02;
pub const BUTTON_MIDDLE: u8 = 0x04;
/// IntelliMouse extra buttons
pub const BUTTON_4: u8 = 0x08;
pub const BUTTON_5: u8 = 0x10;

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

/// Type of mouse event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventType {
    /// Mouse moved
    Move,
    /// Button pressed
    ButtonDown,
    /// Button released
    ButtonUp,
    /// Scroll wheel
    Scroll,
    /// Double-click detected
    DoubleClick,
}

/// Mouse event data
#[derive(Debug, Clone, Copy)]
pub struct MouseEvent {
    pub event_type: MouseEventType,
    pub dx: i16,
    pub dy: i16,
    pub buttons: u8, // bit 0=left, 1=right, 2=middle
    pub scroll: i8,
    pub abs_x: i32,
    pub abs_y: i32,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Mouse state
// ---------------------------------------------------------------------------

/// Mouse protocol variant
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseProtocol {
    /// Standard 3-byte PS/2 (no wheel)
    Standard,
    /// IntelliMouse 4-byte (scroll wheel)
    IntelliMouse,
    /// IntelliMouse Explorer 4-byte (scroll + 2 extra buttons)
    IntelliMouseExplorer,
}

/// Mouse state
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: u8,
    pub screen_w: i32,
    pub screen_h: i32,
    packet: [u8; 4],
    packet_idx: usize,
    protocol: MouseProtocol,
    /// Acceleration enabled
    pub acceleration: bool,
    /// Tick counter (updated via tick())
    tick_count: u64,
    /// Last left-click timestamp (for double-click)
    last_left_click_tick: u64,
    /// Last left-click position
    last_left_click_x: i32,
    last_left_click_y: i32,
    /// Previous button state (for detecting press/release edges)
    prev_buttons: u8,
    /// Sample rate (packets per second)
    sample_rate: u8,
    /// Resolution (counts per mm)
    resolution: u8,
}

impl MouseState {
    pub const fn new() -> Self {
        MouseState {
            x: 512,
            y: 384,
            buttons: 0,
            screen_w: 1024,
            screen_h: 768,
            packet: [0; 4],
            packet_idx: 0,
            protocol: MouseProtocol::Standard,
            acceleration: true,
            tick_count: 0,
            last_left_click_tick: 0,
            last_left_click_x: 0,
            last_left_click_y: 0,
            prev_buttons: 0,
            sample_rate: 100,
            resolution: 4,
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MOUSE: Mutex<MouseState> = Mutex::new(MouseState::new());
static MOUSE_QUEUE: Mutex<VecDeque<MouseEvent>> = Mutex::new(VecDeque::new());

// ---------------------------------------------------------------------------
// PS/2 controller helpers
// ---------------------------------------------------------------------------

/// Wait for PS/2 controller input buffer to be empty
fn wait_input() {
    for _ in 0..100_000 {
        if inb(PS2_CMD) & 0x02 == 0 {
            return;
        }
        io_wait();
    }
}

/// Wait for PS/2 controller output buffer to be full
fn wait_output() {
    for _ in 0..100_000 {
        if inb(PS2_CMD) & 0x01 != 0 {
            return;
        }
        io_wait();
    }
}

/// Send a command to the PS/2 mouse (via port 0xD4)
fn mouse_cmd(cmd: u8) -> u8 {
    wait_input();
    outb(PS2_CMD, 0xD4); // tell controller: next byte is for mouse
    wait_input();
    outb(PS2_DATA, cmd);
    wait_output();
    inb(PS2_DATA) // read ACK or response
}

/// Send a command with a data byte
fn mouse_cmd_data(cmd: u8, data: u8) -> u8 {
    mouse_cmd(cmd);
    wait_input();
    outb(PS2_CMD, 0xD4);
    wait_input();
    outb(PS2_DATA, data);
    wait_output();
    inb(PS2_DATA)
}

/// Read a response byte from the mouse (after a command that returns data)
fn mouse_read() -> u8 {
    wait_output();
    inb(PS2_DATA)
}

// ---------------------------------------------------------------------------
// Acceleration
// ---------------------------------------------------------------------------

/// Apply integer-based acceleration curve to a delta value.
/// Returns the accelerated delta.
fn accelerate(delta: i16) -> i16 {
    let abs_d = if delta < 0 {
        -(delta as i32)
    } else {
        delta as i32
    };
    let scale = if abs_d >= ACCEL_THRESHOLD_3 {
        ACCEL_SCALE_3
    } else if abs_d >= ACCEL_THRESHOLD_2 {
        ACCEL_SCALE_2
    } else if abs_d >= ACCEL_THRESHOLD_1 {
        ACCEL_SCALE_1
    } else {
        256 // 1.0x, no acceleration
    };
    // Fixed-point multiply: delta * scale / 256
    let result = (delta as i32 * scale) / 256;
    // Clamp to i16 range
    if result > i16::MAX as i32 {
        i16::MAX
    } else if result < i16::MIN as i32 {
        i16::MIN
    } else {
        result as i16
    }
}

// ---------------------------------------------------------------------------
// Packet processing
// ---------------------------------------------------------------------------

/// Enqueue a mouse event and mirror motion/button events into the unified
/// evdev ring so that consumers using the evdev API see the same stream.
fn enqueue_event(event: MouseEvent) {
    // Mirror relative motion into evdev queue.
    match event.event_type {
        MouseEventType::Move => {
            crate::input::evdev::inject_rel_motion(event.dx, event.dy);
        }
        MouseEventType::Scroll => {
            // Scroll wheel as REL_WHEEL axis.
            crate::input::evdev::inject_rel_motion(0, event.scroll as i16);
        }
        MouseEventType::ButtonDown | MouseEventType::ButtonUp => {
            // Encode each button as a separate EV_KEY inject.
            // BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112 (Linux convention).
            let pressed = matches!(event.event_type, MouseEventType::ButtonDown);
            if event.buttons & BUTTON_LEFT != 0 {
                crate::input::evdev::inject_key(0x110, pressed);
            }
            if event.buttons & BUTTON_RIGHT != 0 {
                crate::input::evdev::inject_key(0x111, pressed);
            }
            if event.buttons & BUTTON_MIDDLE != 0 {
                crate::input::evdev::inject_key(0x112, pressed);
            }
        }
        _ => {}
    }
    let mut queue = MOUSE_QUEUE.lock();
    if queue.len() >= MOUSE_QUEUE_SIZE {
        queue.pop_front();
    }
    queue.push_back(event);
}

/// Handle a byte from the PS/2 mouse (called from IRQ12 handler)
pub fn handle_byte(byte: u8) {
    let mut mouse = MOUSE.lock();

    // First byte must have bit 3 set (always-1 bit in standard PS/2)
    if mouse.packet_idx == 0 && byte & 0x08 == 0 {
        return; // resync: discard until we see a valid first byte
    }

    let idx = mouse.packet_idx;
    mouse.packet[idx] = byte;
    mouse.packet_idx = mouse.packet_idx.saturating_add(1);

    let packet_size = match mouse.protocol {
        MouseProtocol::Standard => 3,
        MouseProtocol::IntelliMouse | MouseProtocol::IntelliMouseExplorer => 4,
    };

    if mouse.packet_idx < packet_size {
        return;
    }

    // Complete packet received -- decode it
    mouse.packet_idx = 0;

    let flags = mouse.packet[0];
    let mut dx = mouse.packet[1] as i16;
    let mut dy = mouse.packet[2] as i16;

    // Sign extend using the overflow flags in byte 0
    if flags & 0x10 != 0 {
        dx = dx.saturating_sub(256);
    }
    if flags & 0x20 != 0 {
        dy = dy.saturating_sub(256);
    }

    // Check for overflow -- discard if set
    if flags & 0xC0 != 0 {
        return;
    }

    // PS/2 Y axis is inverted (up = negative in PS/2, but we want up = negative screen)
    dy = -dy;

    // Parse scroll wheel and extra buttons from 4th byte
    let (scroll, extra_buttons) = match mouse.protocol {
        MouseProtocol::IntelliMouse => {
            let s = mouse.packet[3] as i8;
            (s, 0u8)
        }
        MouseProtocol::IntelliMouseExplorer => {
            // Bits 0-3: scroll (signed 4-bit), bits 4-5: buttons 4 and 5
            let raw = mouse.packet[3];
            let scroll_raw = (raw & 0x0F) as i8;
            // Sign-extend 4-bit value
            let s = if scroll_raw & 0x08 != 0 {
                scroll_raw | !0x0F_u8 as i8
            } else {
                scroll_raw
            };
            let btns = (raw >> 4) & 0x03;
            (s, btns)
        }
        MouseProtocol::Standard => (0i8, 0u8),
    };

    let base_buttons = flags & 0x07;
    let buttons = base_buttons | (extra_buttons << 3);

    // Apply acceleration if enabled
    if mouse.acceleration {
        dx = accelerate(dx);
        dy = accelerate(dy);
    }

    // Update cursor position with clamping
    mouse.x = (mouse.x + dx as i32).clamp(0, mouse.screen_w - 1);
    mouse.y = (mouse.y + dy as i32).clamp(0, mouse.screen_h - 1);

    let prev_buttons = mouse.prev_buttons;
    mouse.buttons = buttons;
    mouse.prev_buttons = buttons;

    let abs_x = mouse.x;
    let abs_y = mouse.y;
    let tick = mouse.tick_count;

    // Detect button state changes
    let pressed = buttons & !prev_buttons;
    let released = !buttons & prev_buttons;

    // Check for double-click on left button
    let mut double_click = false;
    if pressed & BUTTON_LEFT != 0 {
        let dt = tick.saturating_sub(mouse.last_left_click_tick);
        let dist_x = (abs_x - mouse.last_left_click_x).abs();
        let dist_y = (abs_y - mouse.last_left_click_y).abs();
        if dt <= DOUBLE_CLICK_THRESHOLD
            && dist_x <= DOUBLE_CLICK_DISTANCE
            && dist_y <= DOUBLE_CLICK_DISTANCE
        {
            double_click = true;
        }
        mouse.last_left_click_tick = tick;
        mouse.last_left_click_x = abs_x;
        mouse.last_left_click_y = abs_y;
    }

    drop(mouse);

    // Emit movement event if delta is nonzero
    if dx != 0 || dy != 0 {
        enqueue_event(MouseEvent {
            event_type: MouseEventType::Move,
            dx,
            dy,
            buttons,
            scroll: 0,
            abs_x,
            abs_y,
            timestamp: tick,
        });
    }

    // Emit button-down events
    if pressed != 0 {
        enqueue_event(MouseEvent {
            event_type: MouseEventType::ButtonDown,
            dx: 0,
            dy: 0,
            buttons: pressed,
            scroll: 0,
            abs_x,
            abs_y,
            timestamp: tick,
        });
    }

    // Emit button-up events
    if released != 0 {
        enqueue_event(MouseEvent {
            event_type: MouseEventType::ButtonUp,
            dx: 0,
            dy: 0,
            buttons: released,
            scroll: 0,
            abs_x,
            abs_y,
            timestamp: tick,
        });
    }

    // Emit double-click event
    if double_click {
        enqueue_event(MouseEvent {
            event_type: MouseEventType::DoubleClick,
            dx: 0,
            dy: 0,
            buttons: BUTTON_LEFT,
            scroll: 0,
            abs_x,
            abs_y,
            timestamp: tick,
        });
    }

    // Emit scroll event
    if scroll != 0 {
        enqueue_event(MouseEvent {
            event_type: MouseEventType::Scroll,
            dx: 0,
            dy: 0,
            buttons,
            scroll,
            abs_x,
            abs_y,
            timestamp: tick,
        });
    }
}

// ---------------------------------------------------------------------------
// Tick -- call from timer interrupt to advance internal timestamp
// ---------------------------------------------------------------------------

/// Advance the mouse driver's internal timestamp. Call from timer ISR.
pub fn tick() {
    let mut m = MOUSE.lock();
    m.tick_count = m.tick_count.saturating_add(1);
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Try to enable IntelliMouse protocol by sending the magic sample rate
/// sequence: 200, 100, 80. If the device ID changes to 3 or 4, the
/// mouse supports the extended protocol.
fn detect_intellimouse() -> MouseProtocol {
    // IntelliMouse: set sample rate 200, 100, 80
    mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 200);
    mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 100);
    mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 80);

    // Read device ID
    mouse_cmd(MOUSE_CMD_GET_DEVICE_ID);
    let id1 = mouse_read();

    if id1 == 3 {
        // Try IntelliMouse Explorer: set sample rate 200, 200, 80
        mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 200);
        mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 200);
        mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 80);

        mouse_cmd(MOUSE_CMD_GET_DEVICE_ID);
        let id2 = mouse_read();

        if id2 == 4 {
            return MouseProtocol::IntelliMouseExplorer;
        }
        return MouseProtocol::IntelliMouse;
    }

    MouseProtocol::Standard
}

/// Initialize the PS/2 mouse
pub fn init() {
    // Enable auxiliary device (mouse) on the PS/2 controller
    wait_input();
    outb(PS2_CMD, 0xA8);

    // Enable IRQ12 in the PS/2 controller configuration byte
    wait_input();
    outb(PS2_CMD, 0x20); // read controller config
    wait_output();
    let config = inb(PS2_DATA);
    wait_input();
    outb(PS2_CMD, 0x60); // write controller config
    wait_input();
    outb(PS2_DATA, config | 0x02); // set bit 1 = enable IRQ12

    // Reset mouse
    mouse_cmd(MOUSE_CMD_RESET);
    // After reset, mouse sends 0xAA (self-test pass) then 0x00 (device ID)
    // Give it time
    for _ in 0..50_000 {
        io_wait();
    }
    // Drain any pending bytes
    while inb(PS2_CMD) & 0x01 != 0 {
        let _ = inb(PS2_DATA);
    }

    // Set defaults
    mouse_cmd(MOUSE_CMD_SET_DEFAULTS);

    // Detect extended protocol
    let protocol = detect_intellimouse();
    MOUSE.lock().protocol = protocol;

    // Set sample rate to 100 packets/sec
    mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, 100);
    MOUSE.lock().sample_rate = 100;

    // Set resolution to 8 counts/mm
    mouse_cmd_data(MOUSE_CMD_SET_RESOLUTION, 3); // 0=1, 1=2, 2=4, 3=8 counts/mm
    MOUSE.lock().resolution = 8;

    // Set stream mode
    mouse_cmd(MOUSE_CMD_SET_STREAM_MODE);

    // Enable data reporting
    mouse_cmd(MOUSE_CMD_ENABLE_REPORTING);

    let protocol_name = match protocol {
        MouseProtocol::Standard => "standard 3-byte",
        MouseProtocol::IntelliMouse => "IntelliMouse 4-byte (wheel)",
        MouseProtocol::IntelliMouseExplorer => "IntelliMouse Explorer (wheel+buttons)",
    };

    super::register("ps2-mouse", super::DeviceType::Mouse);
    serial_println!("  Mouse: PS/2 driver ready ({})", protocol_name);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Get current mouse position
pub fn position() -> (i32, i32) {
    let m = MOUSE.lock();
    (m.x, m.y)
}

/// Get current button state
pub fn buttons() -> u8 {
    MOUSE.lock().buttons
}

/// Pop a mouse event from the queue
pub fn pop_event() -> Option<MouseEvent> {
    MOUSE_QUEUE.lock().pop_front()
}

/// Check if there are pending mouse events
pub fn has_events() -> bool {
    !MOUSE_QUEUE.lock().is_empty()
}

/// Get the number of pending mouse events
pub fn pending_count() -> usize {
    MOUSE_QUEUE.lock().len()
}

/// Flush all pending mouse events
pub fn flush_events() {
    MOUSE_QUEUE.lock().clear();
}

/// Set the screen dimensions for cursor clamping
pub fn set_screen_size(width: i32, height: i32) {
    let mut m = MOUSE.lock();
    m.screen_w = width;
    m.screen_h = height;
    // Re-clamp current position
    m.x = m.x.clamp(0, width - 1);
    m.y = m.y.clamp(0, height - 1);
}

/// Move cursor to an absolute position
pub fn set_position(x: i32, y: i32) {
    let mut m = MOUSE.lock();
    m.x = x.clamp(0, m.screen_w - 1);
    m.y = y.clamp(0, m.screen_h - 1);
}

/// Enable or disable mouse acceleration
pub fn set_acceleration(enabled: bool) {
    MOUSE.lock().acceleration = enabled;
}

/// Check if a specific button is currently held
pub fn is_button_down(button: u8) -> bool {
    MOUSE.lock().buttons & button != 0
}

/// Set the PS/2 mouse sample rate (10, 20, 40, 60, 80, 100, 200)
pub fn set_sample_rate(rate: u8) {
    let rate = match rate {
        r if r <= 10 => 10,
        r if r <= 20 => 20,
        r if r <= 40 => 40,
        r if r <= 60 => 60,
        r if r <= 80 => 80,
        r if r <= 100 => 100,
        _ => 200,
    };
    mouse_cmd_data(MOUSE_CMD_SET_SAMPLE_RATE, rate);
    MOUSE.lock().sample_rate = rate;
    serial_println!("  Mouse: sample rate set to {} packets/sec", rate);
}

/// Set the PS/2 mouse resolution
/// 0 = 1 count/mm, 1 = 2 count/mm, 2 = 4 count/mm, 3 = 8 count/mm
pub fn set_resolution(res_code: u8) {
    let code = res_code.min(3);
    mouse_cmd_data(MOUSE_CMD_SET_RESOLUTION, code);
    let counts_per_mm = 1u8 << code;
    MOUSE.lock().resolution = counts_per_mm;
    serial_println!("  Mouse: resolution set to {} counts/mm", counts_per_mm);
}

/// Get the detected mouse protocol
pub fn protocol() -> MouseProtocol {
    MOUSE.lock().protocol
}

/// Get full mouse state snapshot
pub fn state() -> (i32, i32, u8, bool) {
    let m = MOUSE.lock();
    (m.x, m.y, m.buttons, m.acceleration)
}

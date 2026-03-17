use crate::sync::Mutex;
/// Input event subsystem for the Hoags Display Server
///
/// Unified input handling for keyboard, mouse, touchscreen, and gamepad.
/// Events are routed to the focused window or consumed by the shell.
///
/// Inspired by: Linux evdev, Wayland input protocol, Android InputManager.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;

/// Input event types
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// Key press/release
    Key {
        scancode: u8,
        pressed: bool,
        character: Option<char>,
    },
    /// Mouse movement (relative)
    MouseMove { dx: i32, dy: i32 },
    /// Mouse movement (absolute)
    MouseAbsolute { x: i32, y: i32 },
    /// Mouse button press/release
    MouseButton { button: MouseButton, pressed: bool },
    /// Mouse scroll wheel
    MouseScroll { delta: i32 },
    /// Touch event
    Touch {
        id: u32,
        x: i32,
        y: i32,
        phase: TouchPhase,
    },
}

/// Mouse buttons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// Touch event phases
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

/// Input event queue
static INPUT_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());

/// Mouse state
pub struct MouseState {
    pub x: i32,
    pub y: i32,
    pub buttons: [bool; 5], // left, right, middle, back, forward
}

static MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState {
    x: 512,
    y: 384,
    buttons: [false; 5],
});

/// Push an input event (called from interrupt handlers)
pub fn push_event(event: InputEvent) {
    let mut queue = INPUT_QUEUE.lock();
    if queue.len() >= 1024 {
        queue.pop_front();
    }
    queue.push_back(event);

    // Update mouse state
    match event {
        InputEvent::MouseMove { dx, dy } => {
            let mut mouse = MOUSE_STATE.lock();
            mouse.x += dx;
            mouse.y += dy;
            // Clamp to screen bounds
            mouse.x = mouse.x.max(0).min(1920);
            mouse.y = mouse.y.max(0).min(1080);
        }
        InputEvent::MouseAbsolute { x, y } => {
            let mut mouse = MOUSE_STATE.lock();
            mouse.x = x;
            mouse.y = y;
        }
        InputEvent::MouseButton { button, pressed } => {
            let mut mouse = MOUSE_STATE.lock();
            let idx = match button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
                MouseButton::Back => 3,
                MouseButton::Forward => 4,
            };
            mouse.buttons[idx] = pressed;
        }
        _ => {}
    }
}

/// Pop the next input event
pub fn pop_event() -> Option<InputEvent> {
    INPUT_QUEUE.lock().pop_front()
}

/// Get current mouse position
pub fn mouse_position() -> (i32, i32) {
    let state = MOUSE_STATE.lock();
    (state.x, state.y)
}

/// Check if a mouse button is pressed
pub fn mouse_button_pressed(button: MouseButton) -> bool {
    let state = MOUSE_STATE.lock();
    let idx = match button {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
    };
    state.buttons[idx]
}

/// Initialize input subsystem
pub fn init() {
    serial_println!("  Input: event subsystem ready");
}

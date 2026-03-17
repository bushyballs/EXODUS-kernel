/// Gamepad/controller input for Genesis
///
/// Dual analog sticks, buttons, triggers, rumble, up to 4 players.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum GamepadButton {
    A,
    B,
    X,
    Y,
    LB,
    RB,
    LT,
    RT,
    Start,
    Select,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    LeftStick,
    RightStick,
}

#[derive(Clone, Copy)]
struct GamepadState {
    buttons: u16,
    left_stick_x: i16,
    left_stick_y: i16,
    right_stick_x: i16,
    right_stick_y: i16,
    left_trigger: u8,
    right_trigger: u8,
    connected: bool,
    battery: u8,
    rumble_left: u8,
    rumble_right: u8,
}

struct GamepadManager {
    gamepads: [GamepadState; 4],
    connected_count: u8,
    deadzone: u16,
}

static GAMEPAD: Mutex<Option<GamepadManager>> = Mutex::new(None);

impl GamepadManager {
    fn new() -> Self {
        let empty = GamepadState {
            buttons: 0,
            left_stick_x: 0,
            left_stick_y: 0,
            right_stick_x: 0,
            right_stick_y: 0,
            left_trigger: 0,
            right_trigger: 0,
            connected: false,
            battery: 0,
            rumble_left: 0,
            rumble_right: 0,
        };
        GamepadManager {
            gamepads: [empty; 4],
            connected_count: 0,
            deadzone: 4000,
        }
    }

    fn is_pressed(&self, pad: u8, button: GamepadButton) -> bool {
        if pad >= 4 {
            return false;
        }
        let bit = button as u16;
        (self.gamepads[pad as usize].buttons & (1 << bit)) != 0
    }

    fn get_axis(&self, pad: u8, left: bool) -> (i16, i16) {
        if pad >= 4 {
            return (0, 0);
        }
        let g = &self.gamepads[pad as usize];
        let (x, y) = if left {
            (g.left_stick_x, g.left_stick_y)
        } else {
            (g.right_stick_x, g.right_stick_y)
        };
        // Apply deadzone
        let dz = self.deadzone as i16;
        let ax = if x.abs() < dz { 0 } else { x };
        let ay = if y.abs() < dz { 0 } else { y };
        (ax, ay)
    }

    fn set_rumble(&mut self, pad: u8, left: u8, right: u8) {
        if pad < 4 {
            self.gamepads[pad as usize].rumble_left = left;
            self.gamepads[pad as usize].rumble_right = right;
        }
    }

    fn update_state(
        &mut self,
        pad: u8,
        buttons: u16,
        lx: i16,
        ly: i16,
        rx: i16,
        ry: i16,
        lt: u8,
        rt: u8,
    ) {
        if pad >= 4 {
            return;
        }
        let g = &mut self.gamepads[pad as usize];
        g.buttons = buttons;
        g.left_stick_x = lx;
        g.left_stick_y = ly;
        g.right_stick_x = rx;
        g.right_stick_y = ry;
        g.left_trigger = lt;
        g.right_trigger = rt;
        if !g.connected {
            g.connected = true;
            self.connected_count = self.connected_count.saturating_add(1);
        }
    }
}

pub fn init() {
    let mut g = GAMEPAD.lock();
    *g = Some(GamepadManager::new());
    serial_println!("    Gamepad: 4-player, dual analog, rumble ready");
}

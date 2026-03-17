use crate::sync::Mutex;
/// Game Input Mapping for Genesis
///
/// Action-based input abstraction that maps raw key codes and gamepad
/// buttons to logical game actions. Supports rebinding, axis values
/// (Q16 fixed-point), per-frame press/release detection, and
/// save/load of custom bindings.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constants
const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;
const Q16_HALF: i32 = 32768;
const Q16_NEG_ONE: i32 = -65536;

/// Maximum number of input bindings.
const MAX_BINDINGS: usize = 64;

/// Sentinel value meaning "no key/button bound".
const UNBOUND: u32 = 0xFFFFFFFF;

/// Default key codes (matching typical scancode layout).
const KEY_W: u32 = 0x11;
const KEY_A: u32 = 0x1E;
const KEY_S: u32 = 0x1F;
const KEY_D: u32 = 0x20;
const KEY_SPACE: u32 = 0x39;
const KEY_E: u32 = 0x12;
const KEY_F: u32 = 0x21;
const KEY_ESCAPE: u32 = 0x01;
const KEY_ENTER: u32 = 0x1C;
const KEY_TAB: u32 = 0x0F;
const KEY_BACKSPACE: u32 = 0x0E;
const KEY_UP: u32 = 0x48;
const KEY_DOWN: u32 = 0x50;
const KEY_LEFT: u32 = 0x4B;
const KEY_RIGHT: u32 = 0x4D;

/// Default gamepad button codes.
const GAMEPAD_DPAD_UP: u32 = 0x100;
const GAMEPAD_DPAD_DOWN: u32 = 0x101;
const GAMEPAD_DPAD_LEFT: u32 = 0x102;
const GAMEPAD_DPAD_RIGHT: u32 = 0x103;
const GAMEPAD_A: u32 = 0x110;
const GAMEPAD_B: u32 = 0x111;
const GAMEPAD_X: u32 = 0x112;
const GAMEPAD_Y: u32 = 0x113;
const GAMEPAD_START: u32 = 0x120;
const GAMEPAD_SELECT: u32 = 0x121;

/// Logical game actions that can be mapped to physical inputs.
#[derive(Clone, Copy, PartialEq)]
pub enum InputAction {
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    Jump,
    Attack,
    Interact,
    Pause,
    Menu,
    Confirm,
    Cancel,
    Custom(u32),
}

/// A binding maps a logical action to a key code and/or gamepad button.
#[derive(Clone, Copy)]
pub struct InputBinding {
    pub action: InputAction,
    pub key_code: u32,
    pub gamepad_button: u32,
    pub active: bool,
}

/// Per-action state tracked each frame.
#[derive(Clone, Copy)]
pub struct InputState {
    pub pressed: bool,
    pub just_pressed: bool,
    pub just_released: bool,
    pub axis_value: i32, // Q16: -1.0 to 1.0
    prev_pressed: bool,
}

/// Saved binding entry for serialization.
#[derive(Clone, Copy)]
pub struct SavedBinding {
    pub action_id: u32,
    pub key_code: u32,
    pub gamepad_button: u32,
}

/// The input manager holds all bindings and per-frame state.
struct InputManager {
    bindings: Vec<InputBinding>,
    states: Vec<InputState>,
    raw_keys: [bool; 256],
    raw_gamepad: [bool; 64],
    gamepad_axis_x: i32, // Q16 left stick X
    gamepad_axis_y: i32, // Q16 left stick Y
    binding_count: usize,
    enabled: bool,
}

static INPUT_MANAGER: Mutex<Option<InputManager>> = Mutex::new(None);

/// Convert an InputAction to a numeric id for save/load.
fn action_to_id(action: InputAction) -> u32 {
    match action {
        InputAction::MoveUp => 0,
        InputAction::MoveDown => 1,
        InputAction::MoveLeft => 2,
        InputAction::MoveRight => 3,
        InputAction::Jump => 4,
        InputAction::Attack => 5,
        InputAction::Interact => 6,
        InputAction::Pause => 7,
        InputAction::Menu => 8,
        InputAction::Confirm => 9,
        InputAction::Cancel => 10,
        InputAction::Custom(n) => 100 + n,
    }
}

/// Convert a numeric id back to an InputAction.
fn id_to_action(id: u32) -> InputAction {
    match id {
        0 => InputAction::MoveUp,
        1 => InputAction::MoveDown,
        2 => InputAction::MoveLeft,
        3 => InputAction::MoveRight,
        4 => InputAction::Jump,
        5 => InputAction::Attack,
        6 => InputAction::Interact,
        7 => InputAction::Pause,
        8 => InputAction::Menu,
        9 => InputAction::Confirm,
        10 => InputAction::Cancel,
        n => InputAction::Custom(n.saturating_sub(100)),
    }
}

impl InputState {
    fn new() -> Self {
        InputState {
            pressed: false,
            just_pressed: false,
            just_released: false,
            axis_value: Q16_ZERO,
            prev_pressed: false,
        }
    }

    /// Update edge detection from current pressed state.
    fn update_edges(&mut self) {
        self.just_pressed = self.pressed && !self.prev_pressed;
        self.just_released = !self.pressed && self.prev_pressed;
        self.prev_pressed = self.pressed;
    }
}

impl InputManager {
    fn new() -> Self {
        InputManager {
            bindings: Vec::new(),
            states: Vec::new(),
            raw_keys: [false; 256],
            raw_gamepad: [false; 64],
            gamepad_axis_x: Q16_ZERO,
            gamepad_axis_y: Q16_ZERO,
            binding_count: 0,
            enabled: true,
        }
    }

    /// Bind an action to a key code and/or gamepad button.
    fn bind_key(&mut self, action: InputAction, key_code: u32, gamepad_button: u32) -> bool {
        // Check if this action already has a binding and update it
        for (_i, binding) in self.bindings.iter_mut().enumerate() {
            if binding.action == action && binding.active {
                binding.key_code = key_code;
                binding.gamepad_button = gamepad_button;
                return true;
            }
        }

        // Create new binding
        if self.bindings.len() >= MAX_BINDINGS {
            serial_println!("    GameInput: max bindings reached ({})", MAX_BINDINGS);
            return false;
        }

        let binding = InputBinding {
            action,
            key_code,
            gamepad_button,
            active: true,
        };
        self.bindings.push(binding);
        self.states.push(InputState::new());
        self.binding_count = self.binding_count.saturating_add(1);
        true
    }

    /// Remove all bindings for a given action.
    fn unbind(&mut self, action: InputAction) -> bool {
        let mut found = false;
        for binding in self.bindings.iter_mut() {
            if binding.action == action && binding.active {
                binding.active = false;
                binding.key_code = UNBOUND;
                binding.gamepad_button = UNBOUND;
                found = true;
            }
        }
        found
    }

    /// Set a raw key state (called by the keyboard driver).
    fn set_key_state(&mut self, key_code: u32, pressed: bool) {
        if (key_code as usize) < 256 {
            self.raw_keys[key_code as usize] = pressed;
        }
    }

    /// Set a raw gamepad button state (called by the gamepad driver).
    fn set_gamepad_state(&mut self, button: u32, pressed: bool) {
        // Map gamepad button codes to array indices
        let index = if button >= 0x100 && button < 0x140 {
            (button - 0x100) as usize
        } else {
            return;
        };
        if index < 64 {
            self.raw_gamepad[index] = pressed;
        }
    }

    /// Set the gamepad analog stick axes (Q16 values).
    fn set_gamepad_axis(&mut self, axis_x: i32, axis_y: i32) {
        self.gamepad_axis_x = axis_x;
        self.gamepad_axis_y = axis_y;
    }

    /// Poll all bindings, updating pressed/released state from raw input.
    fn poll(&mut self) {
        if !self.enabled {
            return;
        }

        for i in 0..self.bindings.len() {
            if !self.bindings[i].active {
                self.states[i].pressed = false;
                self.states[i].update_edges();
                continue;
            }

            let key = self.bindings[i].key_code;
            let btn = self.bindings[i].gamepad_button;

            let key_pressed = if key != UNBOUND && (key as usize) < 256 {
                self.raw_keys[key as usize]
            } else {
                false
            };

            let gamepad_pressed = if btn != UNBOUND && btn >= 0x100 && btn < 0x140 {
                let idx = (btn - 0x100) as usize;
                if idx < 64 {
                    self.raw_gamepad[idx]
                } else {
                    false
                }
            } else {
                false
            };

            self.states[i].pressed = key_pressed || gamepad_pressed;

            // Compute axis value for directional actions
            let axis = match self.bindings[i].action {
                InputAction::MoveUp => {
                    if self.states[i].pressed {
                        Q16_NEG_ONE
                    } else {
                        Q16_ZERO
                    }
                }
                InputAction::MoveDown => {
                    if self.states[i].pressed {
                        Q16_ONE
                    } else {
                        Q16_ZERO
                    }
                }
                InputAction::MoveLeft => {
                    if self.states[i].pressed {
                        Q16_NEG_ONE
                    } else {
                        Q16_ZERO
                    }
                }
                InputAction::MoveRight => {
                    if self.states[i].pressed {
                        Q16_ONE
                    } else {
                        Q16_ZERO
                    }
                }
                _ => {
                    if self.states[i].pressed {
                        Q16_ONE
                    } else {
                        Q16_ZERO
                    }
                }
            };
            self.states[i].axis_value = axis;

            self.states[i].update_edges();
        }
    }

    /// Check if an action is currently pressed.
    fn is_pressed(&self, action: InputAction) -> bool {
        for (i, binding) in self.bindings.iter().enumerate() {
            if binding.action == action && binding.active {
                return self.states[i].pressed;
            }
        }
        false
    }

    /// Check if an action was just pressed this frame.
    fn is_just_pressed(&self, action: InputAction) -> bool {
        for (i, binding) in self.bindings.iter().enumerate() {
            if binding.action == action && binding.active {
                return self.states[i].just_pressed;
            }
        }
        false
    }

    /// Check if an action was just released this frame.
    fn is_just_released(&self, action: InputAction) -> bool {
        for (i, binding) in self.bindings.iter().enumerate() {
            if binding.action == action && binding.active {
                return self.states[i].just_released;
            }
        }
        false
    }

    /// Get the axis value for an action (Q16: -1.0 to 1.0).
    fn get_axis(&self, action: InputAction) -> i32 {
        for (i, binding) in self.bindings.iter().enumerate() {
            if binding.action == action && binding.active {
                return self.states[i].axis_value;
            }
        }
        Q16_ZERO
    }

    /// Get the combined horizontal axis from MoveLeft/MoveRight.
    fn get_horizontal_axis(&self) -> i32 {
        let left = self.get_axis(InputAction::MoveLeft);
        let right = self.get_axis(InputAction::MoveRight);
        // left is negative, right is positive; sum them
        let combined = left + right;
        // Also blend in gamepad analog stick
        let stick = self.gamepad_axis_x;
        if combined != 0 {
            combined
        } else {
            stick
        }
    }

    /// Get the combined vertical axis from MoveUp/MoveDown.
    fn get_vertical_axis(&self) -> i32 {
        let up = self.get_axis(InputAction::MoveUp);
        let down = self.get_axis(InputAction::MoveDown);
        let combined = up + down;
        let stick = self.gamepad_axis_y;
        if combined != 0 {
            combined
        } else {
            stick
        }
    }

    /// Reset per-frame edge states. Call at the start of each frame
    /// before new input events arrive.
    fn reset_frame(&mut self) {
        // Edge detection is handled in poll() via prev_pressed,
        // but we clear the just_pressed/just_released flags here
        // for safety if poll() is called multiple times.
        for state in self.states.iter_mut() {
            state.just_pressed = false;
            state.just_released = false;
        }
    }

    /// Export all active bindings for saving.
    fn save_bindings(&self) -> Vec<SavedBinding> {
        let mut saved = Vec::new();
        for binding in self.bindings.iter() {
            if binding.active {
                saved.push(SavedBinding {
                    action_id: action_to_id(binding.action),
                    key_code: binding.key_code,
                    gamepad_button: binding.gamepad_button,
                });
            }
        }
        saved
    }

    /// Load bindings from saved data, replacing all current bindings.
    fn load_bindings(&mut self, saved: &[SavedBinding]) {
        self.bindings.clear();
        self.states.clear();
        self.binding_count = 0;

        for entry in saved.iter() {
            let action = id_to_action(entry.action_id);
            self.bind_key(action, entry.key_code, entry.gamepad_button);
        }
    }

    /// Load default WASD + arrow key + gamepad bindings.
    fn load_defaults(&mut self) {
        self.bindings.clear();
        self.states.clear();
        self.binding_count = 0;

        // Movement
        self.bind_key(InputAction::MoveUp, KEY_W, GAMEPAD_DPAD_UP);
        self.bind_key(InputAction::MoveDown, KEY_S, GAMEPAD_DPAD_DOWN);
        self.bind_key(InputAction::MoveLeft, KEY_A, GAMEPAD_DPAD_LEFT);
        self.bind_key(InputAction::MoveRight, KEY_D, GAMEPAD_DPAD_RIGHT);

        // Arrow key alternatives (additional bindings)
        self.bind_key(InputAction::Custom(0), KEY_UP, UNBOUND); // alt up
        self.bind_key(InputAction::Custom(1), KEY_DOWN, UNBOUND); // alt down
        self.bind_key(InputAction::Custom(2), KEY_LEFT, UNBOUND); // alt left
        self.bind_key(InputAction::Custom(3), KEY_RIGHT, UNBOUND); // alt right

        // Actions
        self.bind_key(InputAction::Jump, KEY_SPACE, GAMEPAD_A);
        self.bind_key(InputAction::Attack, KEY_F, GAMEPAD_X);
        self.bind_key(InputAction::Interact, KEY_E, GAMEPAD_Y);

        // UI
        self.bind_key(InputAction::Pause, KEY_ESCAPE, GAMEPAD_START);
        self.bind_key(InputAction::Menu, KEY_TAB, GAMEPAD_SELECT);
        self.bind_key(InputAction::Confirm, KEY_ENTER, GAMEPAD_A);
        self.bind_key(InputAction::Cancel, KEY_BACKSPACE, GAMEPAD_B);
    }

    /// Enable or disable all input processing.
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Get the total number of active bindings.
    fn active_binding_count(&self) -> usize {
        self.bindings.iter().filter(|b| b.active).count()
    }
}

// --- Public API ---

/// Bind an action to a key code and gamepad button.
pub fn bind_key(action: InputAction, key_code: u32, gamepad_button: u32) -> bool {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.bind_key(action, key_code, gamepad_button)
    } else {
        false
    }
}

/// Remove all bindings for an action.
pub fn unbind(action: InputAction) -> bool {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.unbind(action)
    } else {
        false
    }
}

/// Check if an action is currently held down.
pub fn is_pressed(action: InputAction) -> bool {
    let mgr = INPUT_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.is_pressed(action)
    } else {
        false
    }
}

/// Check if an action was just pressed this frame.
pub fn is_just_pressed(action: InputAction) -> bool {
    let mgr = INPUT_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.is_just_pressed(action)
    } else {
        false
    }
}

/// Get the axis value for an action (Q16).
pub fn get_axis(action: InputAction) -> i32 {
    let mgr = INPUT_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.get_axis(action)
    } else {
        Q16_ZERO
    }
}

/// Poll all input bindings and update state.
pub fn poll() {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.poll();
    }
}

/// Reset per-frame edge detection flags.
pub fn reset_frame() {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.reset_frame();
    }
}

/// Export bindings for saving to persistent storage.
pub fn save_bindings() -> Vec<SavedBinding> {
    let mgr = INPUT_MANAGER.lock();
    if let Some(ref m) = *mgr {
        m.save_bindings()
    } else {
        Vec::new()
    }
}

/// Load default input bindings (WASD + gamepad).
pub fn load_defaults() {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.load_defaults();
    }
}

/// Feed a raw key event into the input system.
pub fn feed_key(key_code: u32, pressed: bool) {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.set_key_state(key_code, pressed);
    }
}

/// Feed a raw gamepad button event into the input system.
pub fn feed_gamepad_button(button: u32, pressed: bool) {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.set_gamepad_state(button, pressed);
    }
}

/// Feed gamepad analog stick axes (Q16 values).
pub fn feed_gamepad_axis(axis_x: i32, axis_y: i32) {
    let mut mgr = INPUT_MANAGER.lock();
    if let Some(ref mut m) = *mgr {
        m.set_gamepad_axis(axis_x, axis_y);
    }
}

pub fn init() {
    let mut mgr = INPUT_MANAGER.lock();
    *mgr = Some(InputManager::new());

    // Load defaults immediately
    if let Some(ref mut m) = *mgr {
        m.load_defaults();
    }

    serial_println!("    Game input: action mapping, WASD+gamepad defaults, rebindable");
}

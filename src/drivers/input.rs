/// Input subsystem for Genesis — unified input event handling
///
/// Provides a unified interface for all input devices: keyboard, mouse,
/// touchscreen, touchpad, gamepad, stylus, accelerometer.
///
/// Inspired by: Linux input subsystem (drivers/input/). All code is original.
use crate::sync::Mutex;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

/// Input event types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// Synchronization event (report boundary)
    Syn,
    /// Key/button event
    Key,
    /// Relative axis (mouse movement)
    Rel,
    /// Absolute axis (touchscreen, stylus)
    Abs,
    /// Miscellaneous
    Misc,
    /// Switch (lid, headphone jack)
    Switch,
    /// LED (capslock, numlock)
    Led,
    /// Force feedback
    ForceFeedback,
}

/// Input event
#[derive(Debug, Clone, Copy)]
pub struct InputEvent {
    /// Event type
    pub event_type: EventType,
    /// Event code (which key/axis/button)
    pub code: u16,
    /// Event value (0/1 for keys, delta for relative, position for absolute)
    pub value: i32,
    /// Timestamp (ms since boot)
    pub timestamp: u64,
    /// Device ID
    pub device_id: u8,
}

/// Absolute axis info
#[derive(Debug, Clone, Copy)]
pub struct AbsInfo {
    pub minimum: i32,
    pub maximum: i32,
    pub fuzz: i32,
    pub flat: i32,
    pub resolution: i32,
    pub value: i32,
}

/// Input device capabilities
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputDeviceType {
    Keyboard,
    Mouse,
    Touchscreen,
    Touchpad,
    Gamepad,
    Stylus,
    Accelerometer,
    Gyroscope,
}

/// Registered input device
pub struct InputDevice {
    pub id: u8,
    pub name: String,
    pub dev_type: InputDeviceType,
    pub vendor: u16,
    pub product: u16,
    pub version: u16,
    /// Supported event types
    pub capabilities: Vec<EventType>,
    /// For absolute devices: axis info
    pub abs_info: Vec<(u16, AbsInfo)>,
    pub active: bool,
}

/// Multi-touch slot (for multi-touch touchscreens)
#[derive(Debug, Clone, Copy)]
pub struct TouchSlot {
    pub id: i32,
    pub x: i32,
    pub y: i32,
    pub pressure: i32,
    pub major: i32,
    pub minor: i32,
    pub active: bool,
}

/// Input subsystem
pub struct InputSubsystem {
    devices: Vec<InputDevice>,
    event_queue: VecDeque<InputEvent>,
    next_id: u8,
    /// Multi-touch slots (10 fingers)
    touch_slots: [TouchSlot; 10],
    /// Current touch slot index
    current_slot: usize,
}

impl InputSubsystem {
    const fn new() -> Self {
        const EMPTY_SLOT: TouchSlot = TouchSlot {
            id: -1,
            x: 0,
            y: 0,
            pressure: 0,
            major: 0,
            minor: 0,
            active: false,
        };
        InputSubsystem {
            devices: Vec::new(),
            event_queue: VecDeque::new(),
            next_id: 0,
            touch_slots: [EMPTY_SLOT; 10],
            current_slot: 0,
        }
    }

    /// Register an input device
    pub fn register(&mut self, name: &str, dev_type: InputDeviceType) -> u8 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.devices.push(InputDevice {
            id,
            name: String::from(name),
            dev_type,
            vendor: 0,
            product: 0,
            version: 0,
            capabilities: Vec::new(),
            abs_info: Vec::new(),
            active: true,
        });

        // Register in device model
        let class = match dev_type {
            InputDeviceType::Keyboard => "input",
            InputDeviceType::Mouse => "input",
            InputDeviceType::Touchscreen => "input",
            InputDeviceType::Touchpad => "input",
            InputDeviceType::Gamepad => "input",
            _ => "input",
        };
        crate::kernel::device_model::register_device(
            name,
            crate::kernel::device_model::DeviceType::Input,
            "platform",
            class,
        );

        id
    }

    /// Report an input event
    pub fn report_event(&mut self, device_id: u8, event_type: EventType, code: u16, value: i32) {
        let event = InputEvent {
            event_type,
            code,
            value,
            timestamp: crate::time::clock::uptime_ms(),
            device_id,
        };
        self.event_queue.push_back(event);

        // Handle multi-touch tracking
        if event_type == EventType::Abs {
            match code {
                0x2F => {
                    // ABS_MT_SLOT
                    if (value as usize) < 10 {
                        self.current_slot = value as usize;
                    }
                }
                0x39 => {
                    // ABS_MT_TRACKING_ID
                    let slot = &mut self.touch_slots[self.current_slot];
                    if value >= 0 {
                        slot.id = value;
                        slot.active = true;
                    } else {
                        slot.active = false;
                        slot.id = -1;
                    }
                }
                0x35 => {
                    // ABS_MT_POSITION_X
                    self.touch_slots[self.current_slot].x = value;
                }
                0x36 => {
                    // ABS_MT_POSITION_Y
                    self.touch_slots[self.current_slot].y = value;
                }
                0x3A => {
                    // ABS_MT_PRESSURE
                    self.touch_slots[self.current_slot].pressure = value;
                }
                _ => {}
            }
        }
    }

    /// Report a synchronization event (end of event batch)
    pub fn report_sync(&mut self, device_id: u8) {
        self.report_event(device_id, EventType::Syn, 0, 0);
    }

    /// Pop next event
    pub fn pop_event(&mut self) -> Option<InputEvent> {
        self.event_queue.pop_front()
    }

    /// Get active touch points
    pub fn active_touches(&self) -> Vec<(i32, i32, i32)> {
        self.touch_slots
            .iter()
            .filter(|s| s.active)
            .map(|s| (s.x, s.y, s.pressure))
            .collect()
    }

    /// List registered devices
    pub fn list_devices(&self) -> Vec<(u8, String, InputDeviceType)> {
        self.devices
            .iter()
            .filter(|d| d.active)
            .map(|d| (d.id, d.name.clone(), d.dev_type))
            .collect()
    }
}

static INPUT: Mutex<InputSubsystem> = Mutex::new(InputSubsystem::new());

pub fn init() {
    let mut input = INPUT.lock();
    // Register built-in PS/2 devices
    input.register("AT keyboard", InputDeviceType::Keyboard);
    input.register("PS/2 mouse", InputDeviceType::Mouse);
    crate::serial_println!("  [input] Input subsystem initialized");
}

pub fn report_event(dev: u8, etype: EventType, code: u16, value: i32) {
    INPUT.lock().report_event(dev, etype, code, value);
}
pub fn report_sync(dev: u8) {
    INPUT.lock().report_sync(dev);
}
pub fn pop_event() -> Option<InputEvent> {
    INPUT.lock().pop_event()
}

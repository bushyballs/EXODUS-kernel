use crate::sync::Mutex;
/// Event device interface (unified input events)
///
/// Part of the AIOS hardware layer.
/// Implements a unified input event subsystem inspired by Linux evdev.
/// Provides event queuing, dispatch to listeners, timestamping,
/// event filtering, and replay/injection capabilities.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Linux-compatible evdev event type constants
// ---------------------------------------------------------------------------

/// Synchronization event — marks end of a logical event group
pub const EV_SYN: u16 = 0x00;
/// Key / button event
pub const EV_KEY: u16 = 0x01;
/// Relative axis (mouse motion, scroll wheel)
pub const EV_REL: u16 = 0x02;
/// Absolute axis (touchscreen, joystick)
pub const EV_ABS: u16 = 0x03;
/// Miscellaneous event
pub const EV_MSC: u16 = 0x04;
/// LED state event
pub const EV_LED: u16 = 0x11;
/// Sound / beeper event
pub const EV_SND: u16 = 0x12;
/// Auto-repeat event
pub const EV_REP: u16 = 0x14;

// Relative axis codes
pub const REL_X: u16 = 0x00;
pub const REL_Y: u16 = 0x01;
pub const REL_WHEEL: u16 = 0x08;

// Absolute axis codes
pub const ABS_X: u16 = 0x00;
pub const ABS_Y: u16 = 0x01;
pub const ABS_PRESSURE: u16 = 0x18;

// Key/button value constants
pub const KEY_RELEASED: i32 = 0;
pub const KEY_PRESSED: i32 = 1;
pub const KEY_REPEAT: i32 = 2;

// Sync event codes
pub const SYN_REPORT: u16 = 0x00;

// ---------------------------------------------------------------------------
// Linux-compatible evdev InputEvent structure
// ---------------------------------------------------------------------------

/// Linux-compatible evdev input event.
/// Layout matches `struct input_event` in <linux/input.h>.
#[repr(C)]
pub struct LinuxInputEvent {
    /// Seconds component of event timestamp
    pub time_sec: u64,
    /// Microseconds component of event timestamp
    pub time_usec: u64,
    /// Event type (EV_KEY, EV_REL, EV_ABS, …)
    pub event_type: u16,
    /// Event-type-specific code (key code, axis code, …)
    pub code: u16,
    /// Value — for keys: 0 = release, 1 = press, 2 = repeat; for axes: delta or absolute position
    pub value: i32,
}

// ---------------------------------------------------------------------------
// Internal event types (used by the ring-buffer subsystem)
// ---------------------------------------------------------------------------

/// Input event types
#[derive(Clone, Copy, PartialEq)]
pub enum EventType {
    Key,
    RelativeAxis,
    AbsoluteAxis,
    Misc,
    Sync,
}

/// A single input event (internal representation)
pub struct InputEvent {
    pub event_type: EventType,
    pub code: u16,
    pub value: i32,
    pub timestamp_ns: u64,
}

/// Input device capability descriptor
#[derive(Clone)]
pub struct DeviceCapabilities {
    /// Device identifier
    pub device_id: u16,
    /// Human-readable device name
    pub name: [u8; 32],
    pub name_len: u8,
    /// Supported event types (bitfield)
    pub supported_events: u8,
    /// Number of supported key codes
    pub num_keys: u16,
    /// Number of supported axes
    pub num_axes: u8,
    /// Whether device is grabbed (exclusive access)
    pub grabbed: bool,
}

impl DeviceCapabilities {
    fn new(device_id: u16) -> Self {
        DeviceCapabilities {
            device_id,
            name: [0u8; 32],
            name_len: 0,
            supported_events: 0,
            num_keys: 0,
            num_axes: 0,
            grabbed: false,
        }
    }

    fn set_name(&mut self, n: &[u8]) {
        let len = n.len().min(32);
        self.name[..len].copy_from_slice(&n[..len]);
        self.name_len = len as u8;
    }

    fn supports(&self, event_type: EventType) -> bool {
        let bit = match event_type {
            EventType::Key => 0,
            EventType::RelativeAxis => 1,
            EventType::AbsoluteAxis => 2,
            EventType::Misc => 3,
            EventType::Sync => 4,
        };
        (self.supported_events & (1 << bit)) != 0
    }

    fn add_support(&mut self, event_type: EventType) {
        let bit = match event_type {
            EventType::Key => 0,
            EventType::RelativeAxis => 1,
            EventType::AbsoluteAxis => 2,
            EventType::Misc => 3,
            EventType::Sync => 4,
        };
        self.supported_events |= 1 << bit;
    }
}

/// Event filter predicate
pub struct EventFilter {
    /// If set, only pass events of this type
    pub event_type: Option<EventType>,
    /// If set, only pass events with code in this range
    pub code_min: u16,
    pub code_max: u16,
    /// Whether filter is enabled
    pub enabled: bool,
}

impl EventFilter {
    fn new() -> Self {
        EventFilter {
            event_type: None,
            code_min: 0,
            code_max: u16::MAX,
            enabled: false,
        }
    }

    fn matches(&self, event: &InputEvent) -> bool {
        if !self.enabled {
            return true; // disabled filter passes everything
        }
        if let Some(ref et) = self.event_type {
            if !event_type_eq(et, &event.event_type) {
                return false;
            }
        }
        if event.code < self.code_min || event.code > self.code_max {
            return false;
        }
        true
    }
}

fn event_type_eq(a: &EventType, b: &EventType) -> bool {
    match (a, b) {
        (EventType::Key, EventType::Key) => true,
        (EventType::RelativeAxis, EventType::RelativeAxis) => true,
        (EventType::AbsoluteAxis, EventType::AbsoluteAxis) => true,
        (EventType::Misc, EventType::Misc) => true,
        (EventType::Sync, EventType::Sync) => true,
        _ => false,
    }
}

/// Ring buffer for event storage with overflow tracking
struct EventRingBuffer {
    events: Vec<InputEvent>,
    capacity: usize,
    head: usize,
    count: usize,
    overflow_count: u64,
}

impl EventRingBuffer {
    fn new(capacity: usize) -> Self {
        let mut events = Vec::with_capacity(capacity);
        // Pre-fill with dummy events to allow index-based access
        for _ in 0..capacity {
            events.push(InputEvent {
                event_type: EventType::Misc,
                code: 0,
                value: 0,
                timestamp_ns: 0,
            });
        }
        EventRingBuffer {
            events,
            capacity,
            head: 0,
            count: 0,
            overflow_count: 0,
        }
    }

    fn push(&mut self, event: InputEvent) {
        let idx = (self.head + self.count) % self.capacity;
        self.events[idx] = event;
        if self.count < self.capacity {
            self.count = self.count.saturating_add(1);
        } else {
            // Overflow: advance head, losing oldest event
            self.head = (self.head + 1) % self.capacity;
            self.overflow_count = self.overflow_count.saturating_add(1);
        }
    }

    fn drain_all(&mut self) -> Vec<InputEvent> {
        let mut result = Vec::with_capacity(self.count);
        for i in 0..self.count {
            let idx = (self.head + i) % self.capacity;
            // Move events out by creating new ones with same data
            result.push(InputEvent {
                event_type: copy_event_type(&self.events[idx].event_type),
                code: self.events[idx].code,
                value: self.events[idx].value,
                timestamp_ns: self.events[idx].timestamp_ns,
            });
        }
        self.head = 0;
        self.count = 0;
        result
    }

    fn len(&self) -> usize {
        self.count
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn clear(&mut self) {
        self.head = 0;
        self.count = 0;
    }

    /// Pop the oldest (front) event from the ring buffer.
    /// Returns `None` if the buffer is empty.
    fn pop_front(&mut self) -> Option<InputEvent> {
        if self.count == 0 {
            return None;
        }
        let idx = self.head;
        let ev = InputEvent {
            event_type: copy_event_type(&self.events[idx].event_type),
            code: self.events[idx].code,
            value: self.events[idx].value,
            timestamp_ns: self.events[idx].timestamp_ns,
        };
        self.head = (self.head + 1) % self.capacity;
        self.count = self.count.saturating_sub(1);
        Some(ev)
    }
}

fn copy_event_type(et: &EventType) -> EventType {
    match et {
        EventType::Key => EventType::Key,
        EventType::RelativeAxis => EventType::RelativeAxis,
        EventType::AbsoluteAxis => EventType::AbsoluteAxis,
        EventType::Misc => EventType::Misc,
        EventType::Sync => EventType::Sync,
    }
}

/// The evdev subsystem state
struct EvdevSubsystem {
    /// Primary event ring buffer
    ring: EventRingBuffer,
    /// Registered input devices
    devices: Vec<DeviceCapabilities>,
    /// Event filters
    filters: Vec<EventFilter>,
    /// Global event counter (monotonic)
    total_events: u64,
    /// Timestamp source: simple monotonic counter (increments per event)
    timestamp_counter: u64,
    /// Whether the subsystem is initialized
    initialized: bool,
}

impl EvdevSubsystem {
    fn new() -> Self {
        EvdevSubsystem {
            ring: EventRingBuffer::new(256),
            devices: Vec::new(),
            filters: Vec::new(),
            total_events: 0,
            timestamp_counter: 0,
            initialized: true,
        }
    }

    /// Register a new input device, returns device id
    fn register_device(&mut self, name: &[u8], events: u8, keys: u16, axes: u8) -> u16 {
        let id = self.devices.len() as u16;
        let mut caps = DeviceCapabilities::new(id);
        caps.set_name(name);
        caps.supported_events = events;
        caps.num_keys = keys;
        caps.num_axes = axes;
        self.devices.push(caps);
        serial_println!(
            "    [evdev] registered device {} (id={})",
            core::str::from_utf8(&name[..name.len().min(32)]).unwrap_or("?"),
            id
        );
        id
    }

    /// Add an event filter
    fn add_filter(&mut self, filter: EventFilter) {
        self.filters.push(filter);
    }

    /// Push an event through filters into the ring buffer
    fn push_event(&mut self, mut event: InputEvent) {
        // Assign timestamp if not provided
        if event.timestamp_ns == 0 {
            self.timestamp_counter = self.timestamp_counter.saturating_add(1_000_000); // 1ms increments
            event.timestamp_ns = self.timestamp_counter;
        }

        // Check filters
        for f in &self.filters {
            if !f.matches(&event) {
                return; // Filtered out
            }
        }

        self.ring.push(event);
        self.total_events = self.total_events.saturating_add(1);
    }

    /// Drain all pending events
    fn poll_events(&mut self) -> Vec<InputEvent> {
        self.ring.drain_all()
    }

    /// Get total event count since init
    fn total_events(&self) -> u64 {
        self.total_events
    }

    /// Get number of pending events
    fn pending_count(&self) -> usize {
        self.ring.len()
    }

    /// Get overflow count
    fn overflow_count(&self) -> u64 {
        self.ring.overflow_count
    }

    /// Get number of registered devices
    fn device_count(&self) -> usize {
        self.devices.len()
    }

    /// Grab a device for exclusive access
    fn grab_device(&mut self, device_id: u16) -> bool {
        for dev in &mut self.devices {
            if dev.device_id == device_id {
                dev.grabbed = true;
                return true;
            }
        }
        false
    }

    /// Release an exclusively grabbed device
    fn ungrab_device(&mut self, device_id: u16) -> bool {
        for dev in &mut self.devices {
            if dev.device_id == device_id {
                dev.grabbed = false;
                return true;
            }
        }
        false
    }

    /// Flush/clear all pending events
    fn flush(&mut self) {
        self.ring.clear();
    }

    /// Inject a synthetic event (for testing/automation)
    fn inject_event(&mut self, event_type: EventType, code: u16, value: i32) {
        self.push_event(InputEvent {
            event_type,
            code,
            value,
            timestamp_ns: 0, // Will be auto-assigned
        });
    }
}

static EVDEV: Mutex<Option<EvdevSubsystem>> = Mutex::new(None);

// Legacy static for backward compatibility with existing push_event callers
static EVENT_QUEUE: Mutex<Vec<InputEvent>> = Mutex::new(Vec::new());

/// Push an input event into the evdev subsystem
pub fn push_event(event: InputEvent) {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.push_event(event);
    } else {
        // Fallback to simple queue if not initialized
        EVENT_QUEUE.lock().push(event);
    }
}

/// Poll and drain all pending events
pub fn poll_events() -> Vec<InputEvent> {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.poll_events()
    } else {
        // Fallback: drain legacy queue
        let mut q = EVENT_QUEUE.lock();
        let mut result = Vec::with_capacity(q.len());
        while let Some(e) = q.pop() {
            result.push(e);
        }
        result.reverse(); // pop reverses order, fix it
        result
    }
}

/// Register a new input device
pub fn register_device(name: &[u8], events: u8, keys: u16, axes: u8) -> u16 {
    let mut guard = EVDEV.lock();
    match guard.as_mut() {
        Some(subsys) => subsys.register_device(name, events, keys, axes),
        None => 0,
    }
}

/// Get number of pending events
pub fn pending_count() -> usize {
    let guard = EVDEV.lock();
    match guard.as_ref() {
        Some(subsys) => subsys.pending_count(),
        None => EVENT_QUEUE.lock().len(),
    }
}

/// Get total events processed since init
pub fn total_events() -> u64 {
    let guard = EVDEV.lock();
    match guard.as_ref() {
        Some(subsys) => subsys.total_events(),
        None => 0,
    }
}

/// Inject a synthetic event for testing
pub fn inject_event(event_type: EventType, code: u16, value: i32) {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.inject_event(event_type, code, value);
    }
}

/// Flush all pending events
pub fn flush() {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.flush();
    } else {
        EVENT_QUEUE.lock().clear();
    }
}

/// Pop a single event from the front of the queue.
/// Returns `None` if the queue is empty.
/// Pop the oldest event from the evdev ring buffer.
///
/// Returns `None` if the queue is currently empty.  Callers can loop until
/// `None` to drain all pending events or call `poll_events()` to take the
/// entire batch at once.
pub fn pop_event() -> Option<InputEvent> {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.ring.pop_front()
    } else {
        // Fallback legacy queue: pop oldest (front).
        let mut q = EVENT_QUEUE.lock();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    }
}

/// Inject a key press or release event into the evdev ring.
///
/// `key_code` — Linux key code (e.g., KEY_A = 30, KEY_ENTER = 28).
/// `pressed`  — `true` for key-down (value=1), `false` for key-up (value=0).
pub fn inject_key(key_code: u16, pressed: bool) {
    let value = if pressed { KEY_PRESSED } else { KEY_RELEASED };
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.inject_event(EventType::Key, key_code, value);
        // Follow with EV_SYN / SYN_REPORT
        subsys.inject_event(EventType::Sync, SYN_REPORT, 0);
    }
}

/// Inject a relative motion event and a trailing SYN_REPORT.
///
/// Typically used to feed PS/2 mouse deltas into the evdev queue so that
/// consumers see the same unified stream as from a real evdev device.
pub fn inject_rel_motion(dx: i16, dy: i16) {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        if dx != 0 {
            subsys.inject_event(EventType::RelativeAxis, REL_X, dx as i32);
        }
        if dy != 0 {
            subsys.inject_event(EventType::RelativeAxis, REL_Y, dy as i32);
        }
        // Always close the event group with SYN_REPORT.
        subsys.inject_event(EventType::Sync, SYN_REPORT, 0);
    }
}

/// Inject an absolute axis event (e.g., from a touchscreen).
///
/// `axis`  — Axis code (ABS_X, ABS_Y, ABS_PRESSURE, …).
/// `value` — Absolute position / value on that axis.
pub fn inject_abs(axis: u16, value: i32) {
    let mut guard = EVDEV.lock();
    if let Some(subsys) = guard.as_mut() {
        subsys.inject_event(EventType::AbsoluteAxis, axis, value);
        subsys.inject_event(EventType::Sync, SYN_REPORT, 0);
    }
}

/// Initialize the evdev subsystem
pub fn init() {
    let mut guard = EVDEV.lock();
    *guard = Some(EvdevSubsystem::new());
    serial_println!("    [evdev] event device subsystem initialized: 256-event ring buffer");
}

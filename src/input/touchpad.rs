/// Touchpad multi-finger gesture driver
///
/// Part of the AIOS hardware layer.
/// Implements multi-touch tracking, gesture recognition
/// (tap, scroll, pinch, swipe), palm rejection, and
/// edge zone detection for touchpad input devices.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// Gesture types recognized
#[derive(Clone, Copy, PartialEq)]
pub enum Gesture {
    Tap,
    TwoFingerScroll { dx: i32, dy: i32 },
    Pinch { scale: f32 },
    ThreeFingerSwipe { dx: i32, dy: i32 },
}

/// Individual finger tracking slot
#[derive(Clone, Copy)]
struct FingerSlot {
    /// Whether this slot is active (finger touching)
    active: bool,
    /// Current X position
    x: u16,
    /// Current Y position
    y: u16,
    /// Touch-down X position
    start_x: u16,
    /// Touch-down Y position
    start_y: u16,
    /// Contact pressure (0-255)
    pressure: u8,
    /// Contact width (for palm rejection)
    contact_width: u8,
    /// Tracking ID for this contact
    tracking_id: u16,
}

impl FingerSlot {
    fn new() -> Self {
        FingerSlot {
            active: false,
            x: 0,
            y: 0,
            start_x: 0,
            start_y: 0,
            pressure: 0,
            contact_width: 0,
            tracking_id: 0,
        }
    }

    fn activate(&mut self, x: u16, y: u16, tracking_id: u16) {
        self.active = true;
        self.x = x;
        self.y = y;
        self.start_x = x;
        self.start_y = y;
        self.pressure = 128; // default mid-pressure
        self.contact_width = 10; // default contact width
        self.tracking_id = tracking_id;
    }

    fn update(&mut self, x: u16, y: u16, pressure: u8) {
        self.x = x;
        self.y = y;
        self.pressure = pressure;
    }

    fn deactivate(&mut self) {
        self.active = false;
        self.pressure = 0;
    }

    /// Distance moved from start (squared, avoids sqrt)
    fn distance_sq(&self) -> u32 {
        let dx = (self.x as i32 - self.start_x as i32).unsigned_abs();
        let dy = (self.y as i32 - self.start_y as i32).unsigned_abs();
        dx * dx + dy * dy
    }

    /// Delta from start
    fn delta(&self) -> (i32, i32) {
        (
            self.x as i32 - self.start_x as i32,
            self.y as i32 - self.start_y as i32,
        )
    }
}

/// Palm rejection state
#[derive(Clone, Copy, PartialEq)]
enum PalmState {
    NotDetected,
    Suspected,
    Rejected,
}

/// Edge zone identifiers
#[derive(Clone, Copy, PartialEq)]
pub enum EdgeZone {
    None,
    Left,
    Right,
    Top,
    Bottom,
}

/// Gesture recognition state machine
#[derive(Clone, Copy, PartialEq)]
enum GestureState {
    Idle,
    TouchDown,
    Tracking,
    GestureDetected,
}

/// Touchpad device state
pub struct Touchpad {
    pub width: u16,
    pub height: u16,
    pub max_fingers: u8,
    /// Finger tracking slots (up to 5 fingers)
    fingers: [FingerSlot; 5],
    /// Number of currently active fingers
    active_fingers: u8,
    /// Gesture state machine
    gesture_state: GestureState,
    /// Last detected gesture
    last_gesture: Option<Gesture>,
    /// Palm rejection state
    palm_state: PalmState,
    /// Palm rejection threshold (contact width above this = palm)
    palm_width_threshold: u8,
    /// Tap threshold: max distance (squared) for a tap vs. a move
    tap_distance_sq_threshold: u32,
    /// Scroll sensitivity multiplier (x100)
    scroll_sensitivity: u16,
    /// Edge zone width in touchpad units
    edge_zone_width: u16,
    /// Previous distance between two fingers (for pinch)
    prev_two_finger_dist: u32,
    /// Next tracking ID to assign
    next_tracking_id: u16,
    /// Touch-down timestamp (simple counter)
    touch_down_time: u64,
    /// Current time counter
    time_counter: u64,
    /// Tap timeout (in counter units)
    tap_timeout: u64,
    /// Whether the touchpad is enabled
    enabled: bool,
    /// Total gestures detected
    gesture_count: u64,
}

static PAD: Mutex<Option<Touchpad>> = Mutex::new(None);

impl Touchpad {
    fn new(width: u16, height: u16) -> Self {
        Touchpad {
            width,
            height,
            max_fingers: 5,
            fingers: [FingerSlot::new(); 5],
            active_fingers: 0,
            gesture_state: GestureState::Idle,
            last_gesture: None,
            palm_state: PalmState::NotDetected,
            palm_width_threshold: 40,
            tap_distance_sq_threshold: 400, // 20 units squared
            scroll_sensitivity: 100,
            edge_zone_width: 50,
            prev_two_finger_dist: 0,
            next_tracking_id: 1,
            touch_down_time: 0,
            time_counter: 0,
            tap_timeout: 300, // 300ms equivalent
            enabled: true,
            gesture_count: 0,
        }
    }

    /// Process a finger touchdown event
    fn finger_down(&mut self, slot: u8, x: u16, y: u16) {
        if !self.enabled {
            return;
        }
        let s = slot.min(4) as usize;

        // Palm rejection check via position (edges only for large contacts)
        if self.is_in_palm_zone(x, y) {
            self.palm_state = PalmState::Suspected;
        }

        let tid = self.next_tracking_id;
        self.next_tracking_id = self.next_tracking_id.wrapping_add(1);
        self.fingers[s].activate(x, y, tid);

        // Count active fingers
        self.active_fingers = 0;
        for f in &self.fingers {
            if f.active {
                self.active_fingers += 1;
            }
        }

        if self.gesture_state == GestureState::Idle {
            self.gesture_state = GestureState::TouchDown;
            self.touch_down_time = self.time_counter;
        }
    }

    /// Process a finger move event
    fn finger_move(&mut self, slot: u8, x: u16, y: u16, pressure: u8) {
        if !self.enabled {
            return;
        }
        let s = slot.min(4) as usize;
        if !self.fingers[s].active {
            return;
        }

        // Palm rejection via contact width/pressure
        if pressure > 200 || self.fingers[s].contact_width > self.palm_width_threshold {
            self.palm_state = PalmState::Rejected;
            return;
        }

        self.fingers[s].update(x, y, pressure);
        self.gesture_state = GestureState::Tracking;
    }

    /// Process a finger liftoff event
    fn finger_up(&mut self, slot: u8) {
        if !self.enabled {
            return;
        }
        let s = slot.min(4) as usize;
        if !self.fingers[s].active {
            return;
        }

        // Determine gesture before deactivating
        if self.palm_state != PalmState::Rejected {
            self.recognize_gesture(s);
        }

        self.fingers[s].deactivate();

        // Recount active fingers
        self.active_fingers = 0;
        for f in &self.fingers {
            if f.active {
                self.active_fingers += 1;
            }
        }

        if self.active_fingers == 0 {
            self.gesture_state = GestureState::Idle;
            self.palm_state = PalmState::NotDetected;
        }
    }

    /// Attempt gesture recognition based on current state
    fn recognize_gesture(&mut self, released_slot: usize) {
        let finger = &self.fingers[released_slot];
        let dist_sq = finger.distance_sq();
        let elapsed = self.time_counter.saturating_sub(self.touch_down_time);

        match self.active_fingers {
            1 => {
                // Single finger: tap or nothing (swipes handled by move events)
                if dist_sq < self.tap_distance_sq_threshold && elapsed < self.tap_timeout {
                    self.last_gesture = Some(Gesture::Tap);
                    self.gesture_count = self.gesture_count.saturating_add(1);
                }
            }
            2 => {
                // Two fingers: scroll or pinch
                if let Some(gesture) = self.detect_two_finger_gesture() {
                    self.last_gesture = Some(gesture);
                    self.gesture_count = self.gesture_count.saturating_add(1);
                }
            }
            3 => {
                // Three finger swipe
                let (dx, dy) = self.average_delta();
                if abs_i32(dx) > 20 || abs_i32(dy) > 20 {
                    self.last_gesture = Some(Gesture::ThreeFingerSwipe { dx, dy });
                    self.gesture_count = self.gesture_count.saturating_add(1);
                }
            }
            _ => {}
        }

        if self.last_gesture.is_some() {
            self.gesture_state = GestureState::GestureDetected;
        }
    }

    /// Detect two-finger scroll or pinch
    fn detect_two_finger_gesture(&self) -> Option<Gesture> {
        // Find the two active fingers
        let mut active_slots = [0usize; 2];
        let mut count = 0;
        for (i, f) in self.fingers.iter().enumerate() {
            if f.active && count < 2 {
                active_slots[count] = i;
                count += 1;
            }
        }
        if count < 2 {
            return None;
        }

        let f0 = &self.fingers[active_slots[0]];
        let f1 = &self.fingers[active_slots[1]];

        // Check if fingers moved in same direction (scroll) vs apart/together (pinch)
        let (dx0, dy0) = f0.delta();
        let (dx1, dy1) = f1.delta();

        // Same direction = scroll
        let same_dir_x = (dx0 > 0 && dx1 > 0)
            || (dx0 < 0 && dx1 < 0)
            || (abs_i32(dx0) < 10 && abs_i32(dx1) < 10);
        let same_dir_y = (dy0 > 0 && dy1 > 0)
            || (dy0 < 0 && dy1 < 0)
            || (abs_i32(dy0) < 10 && abs_i32(dy1) < 10);

        if same_dir_x && same_dir_y {
            // Scroll gesture
            let avg_dx = (dx0 + dx1) / 2;
            let avg_dy = (dy0 + dy1) / 2;
            let scaled_dx = (avg_dx as i64 * self.scroll_sensitivity as i64 / 100) as i32;
            let scaled_dy = (avg_dy as i64 * self.scroll_sensitivity as i64 / 100) as i32;
            return Some(Gesture::TwoFingerScroll {
                dx: scaled_dx,
                dy: scaled_dy,
            });
        }

        // Opposite direction = pinch
        let cur_dist = finger_distance(f0.x, f0.y, f1.x, f1.y);
        let start_dist = finger_distance(f0.start_x, f0.start_y, f1.start_x, f1.start_y);
        if start_dist > 0 {
            let scale = cur_dist as f32 / start_dist as f32;
            return Some(Gesture::Pinch { scale });
        }

        None
    }

    /// Compute average delta across all active fingers
    fn average_delta(&self) -> (i32, i32) {
        let mut total_dx: i32 = 0;
        let mut total_dy: i32 = 0;
        let mut count: i32 = 0;
        for f in &self.fingers {
            if f.active {
                let (dx, dy) = f.delta();
                total_dx += dx;
                total_dy += dy;
                count += 1;
            }
        }
        if count == 0 {
            return (0, 0);
        }
        (total_dx / count, total_dy / count)
    }

    /// Check if a position is in the palm rejection zone
    fn is_in_palm_zone(&self, _x: u16, y: u16) -> bool {
        // Bottom edge often rests palms
        y > self.height.saturating_sub(self.edge_zone_width / 2)
    }

    /// Detect which edge zone a point is in
    fn edge_zone(&self, x: u16, y: u16) -> EdgeZone {
        let ew = self.edge_zone_width;
        if x < ew {
            return EdgeZone::Left;
        }
        if x > self.width.saturating_sub(ew) {
            return EdgeZone::Right;
        }
        if y < ew {
            return EdgeZone::Top;
        }
        if y > self.height.saturating_sub(ew) {
            return EdgeZone::Bottom;
        }
        EdgeZone::None
    }

    /// Poll for a detected gesture (consumes it)
    fn poll_gesture(&mut self) -> Option<Gesture> {
        self.last_gesture.take()
    }

    /// Set scroll sensitivity (100 = normal, 200 = double speed)
    fn set_scroll_sensitivity(&mut self, sensitivity: u16) {
        self.scroll_sensitivity = sensitivity.max(10).min(500);
    }

    /// Enable or disable the touchpad
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            // Deactivate all fingers
            for f in &mut self.fingers {
                f.deactivate();
            }
            self.active_fingers = 0;
            self.gesture_state = GestureState::Idle;
        }
    }

    /// Advance the internal time counter (called by tick/interrupt handler)
    fn tick(&mut self) {
        self.time_counter = self.time_counter.saturating_add(1);
    }

    /// Get the number of active touch contacts
    fn active_count(&self) -> u8 {
        self.active_fingers
    }

    /// Get total gestures detected since init
    fn total_gestures(&self) -> u64 {
        self.gesture_count
    }
}

/// Compute approximate distance between two points
fn finger_distance(x0: u16, y0: u16, x1: u16, y1: u16) -> u32 {
    let dx = abs_i32(x0 as i32 - x1 as i32) as u32;
    let dy = abs_i32(y0 as i32 - y1 as i32) as u32;
    // Approximate: max(dx,dy) + min(dx,dy)/2
    let max_d = dx.max(dy);
    let min_d = dx.min(dy);
    max_d + min_d / 2
}

fn abs_i32(v: i32) -> i32 {
    if v < 0 {
        -v
    } else {
        v
    }
}

/// Poll for a detected gesture (public API)
pub fn poll_gesture() -> Option<Gesture> {
    let mut guard = PAD.lock();
    match guard.as_mut() {
        Some(pad) => pad.poll_gesture(),
        None => {
            serial_println!("    [touchpad] device not initialized");
            None
        }
    }
}

/// Report finger down
pub fn finger_down(slot: u8, x: u16, y: u16) {
    let mut guard = PAD.lock();
    if let Some(pad) = guard.as_mut() {
        pad.finger_down(slot, x, y);
    }
}

/// Report finger move
pub fn finger_move(slot: u8, x: u16, y: u16, pressure: u8) {
    let mut guard = PAD.lock();
    if let Some(pad) = guard.as_mut() {
        pad.finger_move(slot, x, y, pressure);
    }
}

/// Report finger up
pub fn finger_up(slot: u8) {
    let mut guard = PAD.lock();
    if let Some(pad) = guard.as_mut() {
        pad.finger_up(slot);
    }
}

/// Advance touchpad time
pub fn tick() {
    let mut guard = PAD.lock();
    if let Some(pad) = guard.as_mut() {
        pad.tick();
    }
}

/// Get active finger count
pub fn active_count() -> u8 {
    let guard = PAD.lock();
    match guard.as_ref() {
        Some(pad) => pad.active_count(),
        None => 0,
    }
}

/// Initialize the touchpad subsystem
pub fn init() {
    let mut guard = PAD.lock();
    let pad = Touchpad::new(1920, 1080);
    *guard = Some(pad);
    serial_println!("    [touchpad] initialized: 1920x1080, 5-finger tracking, palm rejection");
}

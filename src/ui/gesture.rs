use crate::sync::Mutex;
/// System-wide gesture recognition
///
/// Part of the AIOS UI layer.
use alloc::vec::Vec;

/// Recognized gesture types
#[derive(Debug, Clone, Copy)]
pub enum GestureType {
    Tap,
    DoubleTap,
    LongPress,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
    Pinch,
    Spread,
}

/// State of a tracked touch point
#[derive(Debug, Clone, Copy)]
struct TouchPoint {
    x: i32,
    y: i32,
    pressed: bool,
    start_x: i32,
    start_y: i32,
}

/// Recognizes gestures from raw touch/pointer input
pub struct GestureRecognizer {
    pub active_points: Vec<(i32, i32)>,
    pub threshold: u32,
    history: Vec<TouchPoint>,
    last_tap_x: i32,
    last_tap_y: i32,
    tap_count: u8,
    hold_ticks: u32,
}

impl GestureRecognizer {
    pub fn new() -> Self {
        GestureRecognizer {
            active_points: Vec::new(),
            threshold: 30,
            history: Vec::new(),
            last_tap_x: 0,
            last_tap_y: 0,
            tap_count: 0,
            hold_ticks: 0,
        }
    }

    /// Set the swipe distance threshold in pixels.
    pub fn set_threshold(&mut self, threshold: u32) {
        self.threshold = threshold;
    }

    /// Feed a raw pointer/touch event into the recognizer.
    pub fn feed_point(&mut self, x: i32, y: i32, pressed: bool) {
        if pressed {
            // Track active point
            if self.history.is_empty() {
                // New gesture starts
                self.history.push(TouchPoint {
                    x,
                    y,
                    pressed,
                    start_x: x,
                    start_y: y,
                });
                self.hold_ticks = 0;
            } else {
                // Update latest point
                if let Some(last) = self.history.last_mut() {
                    last.x = x;
                    last.y = y;
                    self.hold_ticks = self.hold_ticks.saturating_add(1);
                }
            }
            // Update active points list
            self.active_points.clear();
            self.active_points.push((x, y));
        } else {
            // Finger released - finalize gesture data
            if let Some(last) = self.history.last_mut() {
                last.x = x;
                last.y = y;
                last.pressed = false;
            }
        }
    }

    /// Attempt to recognize a gesture from accumulated input.
    ///
    /// Returns `Some(gesture)` if a gesture was recognized, clearing internal state.
    /// Returns `None` if not enough data or no recognizable gesture.
    pub fn recognize(&self) -> Option<GestureType> {
        let tp = self.history.last()?;

        // Only recognize on release
        if tp.pressed {
            // Still pressing - check for long press
            if self.hold_ticks > 30 {
                return Some(GestureType::LongPress);
            }
            return None;
        }

        let dx = tp.x - tp.start_x;
        let dy = tp.y - tp.start_y;
        let dist_sq = (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64);
        let thresh = self.threshold as i64;

        if dist_sq < thresh * thresh {
            // Small movement = tap
            if self.tap_count >= 1 {
                return Some(GestureType::DoubleTap);
            }
            return Some(GestureType::Tap);
        }

        // Determine swipe direction based on dominant axis
        let abs_dx = if dx < 0 { -dx } else { dx };
        let abs_dy = if dy < 0 { -dy } else { dy };

        if abs_dx > abs_dy {
            // Horizontal swipe
            if dx > 0 {
                Some(GestureType::SwipeRight)
            } else {
                Some(GestureType::SwipeLeft)
            }
        } else {
            // Vertical swipe
            if dy > 0 {
                Some(GestureType::SwipeDown)
            } else {
                Some(GestureType::SwipeUp)
            }
        }
    }

    /// Reset the recognizer state, discarding all tracked points.
    pub fn reset(&mut self) {
        self.history.clear();
        self.active_points.clear();
        self.tap_count = 0;
        self.hold_ticks = 0;
    }

    /// Consume the recognized gesture and reset state.
    pub fn consume(&mut self) -> Option<GestureType> {
        let gesture = self.recognize();
        if gesture.is_some() {
            // Track taps for double-tap detection
            if let Some(GestureType::Tap) = gesture {
                if let Some(tp) = self.history.last() {
                    self.last_tap_x = tp.x;
                    self.last_tap_y = tp.y;
                    self.tap_count = self.tap_count.saturating_add(1);
                }
            } else {
                self.tap_count = 0;
            }
            self.history.clear();
            self.active_points.clear();
            self.hold_ticks = 0;
        }
        gesture
    }
}

static GESTURE_RECOGNIZER: Mutex<Option<GestureRecognizer>> = Mutex::new(None);

pub fn init() {
    *GESTURE_RECOGNIZER.lock() = Some(GestureRecognizer::new());
    crate::serial_println!("  [gesture] Gesture recognizer initialized");
}

/// Feed a point into the global gesture recognizer.
pub fn feed_point(x: i32, y: i32, pressed: bool) {
    if let Some(ref mut rec) = *GESTURE_RECOGNIZER.lock() {
        rec.feed_point(x, y, pressed);
    }
}

/// Try to recognize a gesture globally.
pub fn recognize() -> Option<GestureType> {
    match GESTURE_RECOGNIZER.lock().as_ref() {
        Some(rec) => rec.recognize(),
        None => None,
    }
}

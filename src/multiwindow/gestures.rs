use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Touch/mouse gesture types
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    /// Single tap/click
    Tap,
    /// Double tap/click
    DoubleTap,
    /// Long press
    LongPress,
    /// Swipe left
    SwipeLeft,
    /// Swipe right
    SwipeRight,
    /// Swipe up
    SwipeUp,
    /// Swipe down
    SwipeDown,
    /// Pinch to zoom in
    PinchIn,
    /// Pinch to zoom out
    PinchOut,
    /// Two finger drag
    TwoFingerDrag,
    /// Three finger swipe left
    ThreeFingerSwipeLeft,
    /// Three finger swipe right
    ThreeFingerSwipeRight,
    /// Three finger swipe up
    ThreeFingerSwipeUp,
    /// Three finger swipe down
    ThreeFingerSwipeDown,
    /// Four finger tap (show all windows)
    FourFingerTap,
}

/// Touch point
#[derive(Clone, Copy, Debug)]
pub struct TouchPoint {
    pub id: u32,
    pub x: i16,
    pub y: i16,
    pub timestamp_ms: u64,
}

/// Gesture recognizer state
pub struct GestureRecognizer {
    touches: Vec<TouchPoint>,
    last_gesture: Option<Gesture>,
    gesture_start_time: u64,
    double_tap_threshold_ms: u64,
    long_press_threshold_ms: u64,
    swipe_threshold_pixels: u16,
}

impl GestureRecognizer {
    pub fn new() -> Self {
        Self {
            touches: Vec::new(),
            last_gesture: None,
            gesture_start_time: 0,
            double_tap_threshold_ms: 300,
            long_press_threshold_ms: 500,
            swipe_threshold_pixels: 50,
        }
    }

    /// Add a touch point
    pub fn touch_down(&mut self, id: u32, x: i16, y: i16, timestamp_ms: u64) {
        self.touches.push(TouchPoint {
            id,
            x,
            y,
            timestamp_ms,
        });
        if self.touches.len() == 1 {
            self.gesture_start_time = timestamp_ms;
        }
    }

    /// Update touch point position
    pub fn touch_move(&mut self, id: u32, x: i16, y: i16, timestamp_ms: u64) {
        if let Some(touch) = self.touches.iter_mut().find(|t| t.id == id) {
            touch.x = x;
            touch.y = y;
            touch.timestamp_ms = timestamp_ms;
        }
    }

    /// Remove a touch point and potentially recognize gesture
    pub fn touch_up(&mut self, id: u32, timestamp_ms: u64) -> Option<Gesture> {
        if let Some(pos) = self.touches.iter().position(|t| t.id == id) {
            let touch = self.touches[pos];
            self.touches.remove(pos);

            // If all touches are released, recognize gesture
            if self.touches.is_empty() {
                return self.recognize_gesture(touch, timestamp_ms);
            }
        }
        None
    }

    /// Recognize gesture from touch history
    fn recognize_gesture(
        &mut self,
        _final_touch: TouchPoint,
        timestamp_ms: u64,
    ) -> Option<Gesture> {
        let duration = timestamp_ms - self.gesture_start_time;

        // Check for long press
        if duration >= self.long_press_threshold_ms {
            self.last_gesture = Some(Gesture::LongPress);
            return Some(Gesture::LongPress);
        }

        // Check for double tap
        if let Some(Gesture::Tap) = self.last_gesture {
            if duration < self.double_tap_threshold_ms {
                self.last_gesture = None;
                return Some(Gesture::DoubleTap);
            }
        }

        // Simple tap
        self.last_gesture = Some(Gesture::Tap);
        Some(Gesture::Tap)
    }

    /// Detect swipe gesture from touch movement
    pub fn detect_swipe(&self, start: &TouchPoint, end: &TouchPoint) -> Option<Gesture> {
        let dx = end.x - start.x;
        let dy = end.y - start.y;
        let threshold = self.swipe_threshold_pixels as i16;

        if dx.abs() > dy.abs() {
            // Horizontal swipe
            if dx > threshold {
                Some(Gesture::SwipeRight)
            } else if dx < -threshold {
                Some(Gesture::SwipeLeft)
            } else {
                None
            }
        } else {
            // Vertical swipe
            if dy > threshold {
                Some(Gesture::SwipeDown)
            } else if dy < -threshold {
                Some(Gesture::SwipeUp)
            } else {
                None
            }
        }
    }

    /// Clear gesture state
    pub fn reset(&mut self) {
        self.touches.clear();
        self.last_gesture = None;
    }
}

/// Gesture action mapping
#[derive(Clone, Copy, Debug)]
pub enum GestureAction {
    /// Switch to next window
    NextWindow,
    /// Switch to previous window
    PrevWindow,
    /// Show all windows (overview)
    ShowAllWindows,
    /// Minimize current window
    MinimizeWindow,
    /// Maximize current window
    MaximizeWindow,
    /// Close current window
    CloseWindow,
    /// Snap window to left half
    SnapLeft,
    /// Snap window to right half
    SnapRight,
    /// Enter split screen mode
    EnterSplitScreen,
    /// Exit split screen mode
    ExitSplitScreen,
    /// Show desktop
    ShowDesktop,
    /// No action
    None,
}

/// Map gestures to actions
pub fn map_gesture_to_action(gesture: Gesture) -> GestureAction {
    match gesture {
        Gesture::ThreeFingerSwipeLeft => GestureAction::NextWindow,
        Gesture::ThreeFingerSwipeRight => GestureAction::PrevWindow,
        Gesture::ThreeFingerSwipeUp => GestureAction::ShowAllWindows,
        Gesture::ThreeFingerSwipeDown => GestureAction::ShowDesktop,
        Gesture::FourFingerTap => GestureAction::ShowAllWindows,
        Gesture::SwipeLeft => GestureAction::SnapLeft,
        Gesture::SwipeRight => GestureAction::SnapRight,
        Gesture::DoubleTap => GestureAction::MaximizeWindow,
        _ => GestureAction::None,
    }
}

static GESTURE_RECOGNIZER: Mutex<Option<GestureRecognizer>> = Mutex::new(None);

pub fn init() {
    let mut recognizer = GESTURE_RECOGNIZER.lock();
    *recognizer = Some(GestureRecognizer::new());
    serial_println!("[gestures] Gesture recognizer initialized");
}

/// Process touch down event
pub fn touch_down(id: u32, x: i16, y: i16, timestamp_ms: u64) {
    let mut recognizer = GESTURE_RECOGNIZER.lock();
    if let Some(r) = recognizer.as_mut() {
        r.touch_down(id, x, y, timestamp_ms);
    }
}

/// Process touch move event
pub fn touch_move(id: u32, x: i16, y: i16, timestamp_ms: u64) {
    let mut recognizer = GESTURE_RECOGNIZER.lock();
    if let Some(r) = recognizer.as_mut() {
        r.touch_move(id, x, y, timestamp_ms);
    }
}

/// Process touch up event and get recognized gesture
pub fn touch_up(id: u32, timestamp_ms: u64) -> Option<Gesture> {
    let mut recognizer = GESTURE_RECOGNIZER.lock();
    if let Some(r) = recognizer.as_mut() {
        r.touch_up(id, timestamp_ms)
    } else {
        None
    }
}

/// Reset gesture state
pub fn reset() {
    let mut recognizer = GESTURE_RECOGNIZER.lock();
    if let Some(r) = recognizer.as_mut() {
        r.reset();
    }
}

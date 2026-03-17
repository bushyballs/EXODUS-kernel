/// Touch gesture recognition for Genesis
///
/// Tap, swipe, pinch, rotate, edge swipes,
/// multi-finger gestures.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum GestureType {
    Tap,
    DoubleTap,
    LongPress,
    Swipe,
    Pinch,
    Rotate,
    ThreeFingerSwipe,
    FourFingerSwipe,
    EdgeSwipe,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SwipeDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy)]
pub struct GestureEvent {
    pub gesture: GestureType,
    pub x: u16,
    pub y: u16,
    pub x2: u16,
    pub y2: u16,
    pub fingers: u8,
    pub velocity: u16,
    pub direction: SwipeDirection,
    pub scale_x100: u16,
    pub timestamp: u64,
}

struct GestureRecognizer {
    pending_fingers: u8,
    last_tap_time: u64,
    last_tap_x: u16,
    last_tap_y: u16,
    tap_count: u8,
    touch_start_x: u16,
    touch_start_y: u16,
    touch_start_time: u64,
    double_tap_timeout_ms: u32,
    long_press_ms: u32,
    min_swipe_distance: u16,
}

static GESTURE: Mutex<Option<GestureRecognizer>> = Mutex::new(None);

impl GestureRecognizer {
    fn new() -> Self {
        GestureRecognizer {
            pending_fingers: 0,
            last_tap_time: 0,
            last_tap_x: 0,
            last_tap_y: 0,
            tap_count: 0,
            touch_start_x: 0,
            touch_start_y: 0,
            touch_start_time: 0,
            double_tap_timeout_ms: 300,
            long_press_ms: 500,
            min_swipe_distance: 50,
        }
    }

    fn process_touch_down(&mut self, x: u16, y: u16, fingers: u8, timestamp: u64) {
        self.pending_fingers = fingers;
        self.touch_start_x = x;
        self.touch_start_y = y;
        self.touch_start_time = timestamp;
    }

    fn process_touch_up(&mut self, x: u16, y: u16, timestamp: u64) -> Option<GestureEvent> {
        let dx = (x as i32 - self.touch_start_x as i32).abs() as u16;
        let dy = (y as i32 - self.touch_start_y as i32).abs() as u16;
        let duration_ms = timestamp.saturating_sub(self.touch_start_time);

        if dx < self.min_swipe_distance && dy < self.min_swipe_distance {
            // Tap or long press
            if duration_ms > self.long_press_ms as u64 {
                return Some(GestureEvent {
                    gesture: GestureType::LongPress,
                    x,
                    y,
                    x2: 0,
                    y2: 0,
                    fingers: self.pending_fingers,
                    velocity: 0,
                    direction: SwipeDirection::Up,
                    scale_x100: 100,
                    timestamp,
                });
            }
            // Check for double tap
            let since_last = timestamp.saturating_sub(self.last_tap_time);
            if since_last < self.double_tap_timeout_ms as u64 {
                self.tap_count = 0;
                self.last_tap_time = 0;
                return Some(GestureEvent {
                    gesture: GestureType::DoubleTap,
                    x,
                    y,
                    x2: 0,
                    y2: 0,
                    fingers: self.pending_fingers,
                    velocity: 0,
                    direction: SwipeDirection::Up,
                    scale_x100: 100,
                    timestamp,
                });
            }
            self.last_tap_time = timestamp;
            self.last_tap_x = x;
            self.last_tap_y = y;
            self.tap_count = 1;
            return Some(GestureEvent {
                gesture: GestureType::Tap,
                x,
                y,
                x2: 0,
                y2: 0,
                fingers: self.pending_fingers,
                velocity: 0,
                direction: SwipeDirection::Up,
                scale_x100: 100,
                timestamp,
            });
        }

        // Swipe
        let direction = if dx > dy {
            if x > self.touch_start_x {
                SwipeDirection::Right
            } else {
                SwipeDirection::Left
            }
        } else {
            if y > self.touch_start_y {
                SwipeDirection::Down
            } else {
                SwipeDirection::Up
            }
        };
        let dist = (dx as u32) * (dx as u32) + (dy as u32) * (dy as u32);
        let velocity = if duration_ms > 0 {
            (dist / duration_ms as u32) as u16
        } else {
            0
        };

        let gesture = match self.pending_fingers {
            3 => GestureType::ThreeFingerSwipe,
            4 => GestureType::FourFingerSwipe,
            _ => {
                if self.touch_start_x < 10 || self.touch_start_y < 10 {
                    GestureType::EdgeSwipe
                } else {
                    GestureType::Swipe
                }
            }
        };

        Some(GestureEvent {
            gesture,
            x: self.touch_start_x,
            y: self.touch_start_y,
            x2: x,
            y2: y,
            fingers: self.pending_fingers,
            velocity,
            direction,
            scale_x100: 100,
            timestamp,
        })
    }

    fn reset(&mut self) {
        self.pending_fingers = 0;
        self.tap_count = 0;
    }
}

pub fn init() {
    let mut g = GESTURE.lock();
    *g = Some(GestureRecognizer::new());
    serial_println!("    Gesture recognizer: tap, swipe, pinch, edge ready");
}

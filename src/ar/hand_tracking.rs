/// Hand tracking for Genesis AR/VR
///
/// Hand skeleton, gesture recognition,
/// pinch/grab/point detection, finger tracking.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum HandGesture {
    None,
    Pinch,
    Grab,
    Point,
    OpenPalm,
    Fist,
    ThumbsUp,
    Peace,
    Wave,
}

#[derive(Clone, Copy)]
pub struct HandJoint {
    pub x: i16,
    pub y: i16,
    pub z: i16,
    pub confidence: u8,
}

struct HandState {
    joints: [HandJoint; 21], // 21 joints per hand
    gesture: HandGesture,
    gesture_confidence: u8,
    is_left: bool,
    tracked: bool,
}

struct HandTrackingEngine {
    left_hand: HandState,
    right_hand: HandState,
    enabled: bool,
}

static HAND_TRACKING: Mutex<Option<HandTrackingEngine>> = Mutex::new(None);

impl HandTrackingEngine {
    fn new() -> Self {
        let empty_hand = HandState {
            joints: [HandJoint {
                x: 0,
                y: 0,
                z: 0,
                confidence: 0,
            }; 21],
            gesture: HandGesture::None,
            gesture_confidence: 0,
            is_left: false,
            tracked: false,
        };
        HandTrackingEngine {
            left_hand: HandState {
                is_left: true,
                ..empty_hand
            },
            right_hand: empty_hand,
            enabled: false,
        }
    }

    fn detect_gesture(joints: &[HandJoint; 21]) -> HandGesture {
        // Simple gesture detection from joint positions
        let thumb_tip = joints[4];
        let index_tip = joints[8];
        let middle_tip = joints[12];

        // Pinch: thumb and index close together
        let dx = (thumb_tip.x - index_tip.x).abs();
        let dy = (thumb_tip.y - index_tip.y).abs();
        if dx < 30 && dy < 30 {
            return HandGesture::Pinch;
        }

        // Point: index extended, others curled
        if index_tip.y < joints[6].y && middle_tip.y > joints[10].y {
            return HandGesture::Point;
        }

        HandGesture::None
    }
}

pub fn init() {
    let mut h = HAND_TRACKING.lock();
    *h = Some(HandTrackingEngine::new());
    serial_println!("    AR: hand tracking (21-joint, gestures) ready");
}

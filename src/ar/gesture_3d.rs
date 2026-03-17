use crate::sync::Mutex;
use alloc::vec;
/// 3D gesture recognition for Genesis AR/VR
///
/// Hand skeleton tracking, pose estimation, pinch/grab/swipe detection,
/// gesture classification, velocity tracking, gesture recording/replay.
///
/// Joint positions in millimeters. Q16 fixed-point for velocities and scores.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;
const MAX_GESTURE_HISTORY: usize = 120; // ~2 seconds at 60fps
const JOINT_COUNT: usize = 26; // 21 hand + 5 wrist/arm

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum GestureClass {
    None,
    Pinch,
    Grab,
    Release,
    SwipeLeft,
    SwipeRight,
    SwipeUp,
    SwipeDown,
    Push,
    Pull,
    Rotate,
    Tap,
    DoubleTap,
    Hold,
    Spread,
    Custom(u16),
}

#[derive(Clone, Copy, PartialEq)]
pub enum HandSide {
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PoseState {
    Idle,
    Tracking,
    GestureActive,
    Lost,
}

#[derive(Clone, Copy)]
pub struct Joint3D {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub vx: i32, // Q16 mm/s
    pub vy: i32,
    pub vz: i32,
    pub confidence: u8,
}

#[derive(Clone, Copy)]
pub struct PoseSnapshot {
    pub joints: [Joint3D; JOINT_COUNT],
    pub timestamp_ms: u64,
}

#[derive(Clone, Copy)]
pub struct GestureResult {
    pub class: GestureClass,
    pub confidence_q16: i32,
    pub hand: HandSide,
    pub progress_q16: i32, // 0..Q16_ONE for continuous gestures
    pub velocity_q16: i32, // speed of gesture motion
}

#[derive(Clone, Copy)]
struct PinchState {
    active: bool,
    distance_mm: i32,
    start_distance_mm: i32,
    frames_held: u32,
}

#[derive(Clone, Copy)]
struct SwipeState {
    tracking: bool,
    start_x: i32,
    start_y: i32,
    start_z: i32,
    accumulated_dx: i32,
    accumulated_dy: i32,
    accumulated_dz: i32,
    frames: u32,
}

#[derive(Clone, Copy)]
struct GrabState {
    active: bool,
    curl_score_q16: i32,
    frames_held: u32,
}

#[derive(Clone, Copy)]
struct TapState {
    last_tap_frame: u64,
    tap_count: u8,
    cooldown_frames: u8,
}

// ---------------------------------------------------------------------------
// Hand skeleton
// ---------------------------------------------------------------------------

struct HandSkeleton {
    side: HandSide,
    state: PoseState,
    joints: [Joint3D; JOINT_COUNT],
    prev_joints: [Joint3D; JOINT_COUNT],
    pinch: PinchState,
    swipe: SwipeState,
    grab: GrabState,
    tap: TapState,
    last_gesture: GestureClass,
    gesture_confidence_q16: i32,
}

impl HandSkeleton {
    fn new(side: HandSide) -> Self {
        let empty_joint = Joint3D {
            x: 0,
            y: 0,
            z: 0,
            vx: 0,
            vy: 0,
            vz: 0,
            confidence: 0,
        };
        HandSkeleton {
            side,
            state: PoseState::Idle,
            joints: [empty_joint; JOINT_COUNT],
            prev_joints: [empty_joint; JOINT_COUNT],
            pinch: PinchState {
                active: false,
                distance_mm: 0,
                start_distance_mm: 0,
                frames_held: 0,
            },
            swipe: SwipeState {
                tracking: false,
                start_x: 0,
                start_y: 0,
                start_z: 0,
                accumulated_dx: 0,
                accumulated_dy: 0,
                accumulated_dz: 0,
                frames: 0,
            },
            grab: GrabState {
                active: false,
                curl_score_q16: 0,
                frames_held: 0,
            },
            tap: TapState {
                last_tap_frame: 0,
                tap_count: 0,
                cooldown_frames: 0,
            },
            last_gesture: GestureClass::None,
            gesture_confidence_q16: 0,
        }
    }

    /// Update joint positions and compute velocities
    fn update_joints(&mut self, new_joints: &[Joint3D; JOINT_COUNT], dt_ms: u32) {
        self.prev_joints = self.joints;
        self.joints = *new_joints;
        self.state = PoseState::Tracking;

        if dt_ms > 0 {
            for i in 0..JOINT_COUNT {
                let dx = (self.joints[i].x - self.prev_joints[i].x) as i64;
                let dy = (self.joints[i].y - self.prev_joints[i].y) as i64;
                let dz = (self.joints[i].z - self.prev_joints[i].z) as i64;
                // Velocity in Q16 mm/s: (delta_mm * Q16_ONE * 1000) / dt_ms
                self.joints[i].vx = ((dx * Q16_ONE as i64 * 1000) / dt_ms as i64) as i32;
                self.joints[i].vy = ((dy * Q16_ONE as i64 * 1000) / dt_ms as i64) as i32;
                self.joints[i].vz = ((dz * Q16_ONE as i64 * 1000) / dt_ms as i64) as i32;
            }
        }
    }

    /// Distance between two joints in mm
    fn joint_distance(&self, a: usize, b: usize) -> i32 {
        let dx = (self.joints[a].x - self.joints[b].x) as i64;
        let dy = (self.joints[a].y - self.joints[b].y) as i64;
        let dz = (self.joints[a].z - self.joints[b].z) as i64;
        let dist_sq = dx * dx + dy * dy + dz * dz;
        // Integer square root approximation
        isqrt(dist_sq as u64) as i32
    }

    /// Detect pinch gesture (thumb tip = 4, index tip = 8)
    fn detect_pinch(&mut self, frame: u64) -> Option<GestureResult> {
        let dist = self.joint_distance(4, 8);
        self.pinch.distance_mm = dist;

        let pinch_threshold = 25; // 25mm
        let release_threshold = 50; // 50mm

        if dist < pinch_threshold && !self.pinch.active {
            self.pinch.active = true;
            self.pinch.start_distance_mm = dist;
            self.pinch.frames_held = 0;
            let _ = frame;
        }

        if self.pinch.active {
            self.pinch.frames_held = self.pinch.frames_held.saturating_add(1);
            if dist > release_threshold {
                self.pinch.active = false;
                return Some(GestureResult {
                    class: GestureClass::Release,
                    confidence_q16: Q16_ONE,
                    hand: self.side,
                    progress_q16: Q16_ONE,
                    velocity_q16: 0,
                });
            }
            let conf = (((release_threshold - dist) as i64 * Q16_ONE as i64)
                / (release_threshold - pinch_threshold) as i64) as i32;
            return Some(GestureResult {
                class: GestureClass::Pinch,
                confidence_q16: conf.min(Q16_ONE),
                hand: self.side,
                progress_q16: Q16_ONE,
                velocity_q16: 0,
            });
        }
        None
    }

    /// Compute finger curl score (0 = extended, Q16_ONE = fully curled)
    fn finger_curl_score(&self) -> i32 {
        // Check distances from fingertips to palm (joint 0 = wrist)
        let tips = [8, 12, 16, 20]; // index, middle, ring, pinky tips
        let mut total_curl: i64 = 0;

        for &tip in &tips {
            let dist = self.joint_distance(0, tip) as i64;
            // Fully extended ~180mm, fully curled ~40mm
            let curl = if dist < 40 {
                Q16_ONE as i64
            } else if dist > 180 {
                0
            } else {
                (((180 - dist) * Q16_ONE as i64) / 140)
            };
            total_curl += curl;
        }
        ((total_curl / 4) as i32).min(Q16_ONE)
    }

    /// Detect grab gesture (all fingers curled)
    fn detect_grab(&mut self) -> Option<GestureResult> {
        let curl = self.finger_curl_score();
        self.grab.curl_score_q16 = curl;

        let grab_threshold = (((Q16_ONE as i64) * 75) / 100) as i32;
        let release_threshold = (((Q16_ONE as i64) * 40) / 100) as i32;

        if curl > grab_threshold && !self.grab.active {
            self.grab.active = true;
            self.grab.frames_held = 0;
        }

        if self.grab.active {
            self.grab.frames_held = self.grab.frames_held.saturating_add(1);
            if curl < release_threshold {
                self.grab.active = false;
                return Some(GestureResult {
                    class: GestureClass::Release,
                    confidence_q16: Q16_ONE,
                    hand: self.side,
                    progress_q16: Q16_ONE,
                    velocity_q16: 0,
                });
            }
            return Some(GestureResult {
                class: GestureClass::Grab,
                confidence_q16: curl,
                hand: self.side,
                progress_q16: curl,
                velocity_q16: 0,
            });
        }
        None
    }

    /// Detect swipe gestures from palm/wrist velocity
    fn detect_swipe(&mut self) -> Option<GestureResult> {
        let wrist = &self.joints[0];
        let speed_threshold: i64 = 500 * Q16_ONE as i64; // 500 mm/s in Q16

        let vx = wrist.vx as i64;
        let vy = wrist.vy as i64;
        let vz = wrist.vz as i64;
        let speed_sq = vx * vx + vy * vy + vz * vz;
        let thresh_sq = (speed_threshold * speed_threshold) / ((Q16_ONE as i64) * (Q16_ONE as i64));

        if speed_sq > thresh_sq {
            if !self.swipe.tracking {
                self.swipe.tracking = true;
                self.swipe.start_x = wrist.x;
                self.swipe.start_y = wrist.y;
                self.swipe.start_z = wrist.z;
                self.swipe.accumulated_dx = 0;
                self.swipe.accumulated_dy = 0;
                self.swipe.accumulated_dz = 0;
                self.swipe.frames = 0;
            }
            self.swipe.accumulated_dx += wrist.x - self.prev_joints[0].x;
            self.swipe.accumulated_dy += wrist.y - self.prev_joints[0].y;
            self.swipe.accumulated_dz += wrist.z - self.prev_joints[0].z;
            self.swipe.frames = self.swipe.frames.saturating_add(1);
        } else if self.swipe.tracking && self.swipe.frames > 3 {
            self.swipe.tracking = false;
            let dx = self.swipe.accumulated_dx as i64;
            let dy = self.swipe.accumulated_dy as i64;
            let dz = self.swipe.accumulated_dz as i64;
            let abs_dx = if dx < 0 { -dx } else { dx };
            let abs_dy = if dy < 0 { -dy } else { dy };
            let abs_dz = if dz < 0 { -dz } else { dz };

            let min_distance: i64 = 80; // 80mm minimum swipe distance

            let class = if abs_dx > abs_dy && abs_dx > abs_dz && abs_dx > min_distance {
                if dx > 0 {
                    GestureClass::SwipeRight
                } else {
                    GestureClass::SwipeLeft
                }
            } else if abs_dy > abs_dx && abs_dy > abs_dz && abs_dy > min_distance {
                if dy > 0 {
                    GestureClass::SwipeDown
                } else {
                    GestureClass::SwipeUp
                }
            } else if abs_dz > min_distance {
                if dz > 0 {
                    GestureClass::Pull
                } else {
                    GestureClass::Push
                }
            } else {
                self.swipe.tracking = false;
                return None;
            };

            let total_dist = isqrt((dx * dx + dy * dy + dz * dz) as u64) as i64;
            let vel = if self.swipe.frames > 0 {
                ((total_dist * Q16_ONE as i64 * 60) / self.swipe.frames as i64) as i32
            } else {
                0
            };

            return Some(GestureResult {
                class,
                confidence_q16: Q16_ONE,
                hand: self.side,
                progress_q16: Q16_ONE,
                velocity_q16: vel,
            });
        } else {
            self.swipe.tracking = false;
        }
        None
    }

    /// Detect tap gesture (quick downward poke with index finger)
    fn detect_tap(&mut self, frame: u64) -> Option<GestureResult> {
        if self.tap.cooldown_frames > 0 {
            self.tap.cooldown_frames = self.tap.cooldown_frames.saturating_sub(1);
            return None;
        }
        let index_vy = self.joints[8].vy;
        let tap_speed = 300 * Q16_ONE; // 300 mm/s downward

        if index_vy > tap_speed {
            let is_double = frame.wrapping_sub(self.tap.last_tap_frame) < 20;
            self.tap.last_tap_frame = frame;
            self.tap.cooldown_frames = 10;

            if is_double {
                self.tap.tap_count = 0;
                return Some(GestureResult {
                    class: GestureClass::DoubleTap,
                    confidence_q16: Q16_ONE,
                    hand: self.side,
                    progress_q16: Q16_ONE,
                    velocity_q16: index_vy,
                });
            } else {
                self.tap.tap_count = 1;
                return Some(GestureResult {
                    class: GestureClass::Tap,
                    confidence_q16: (((Q16_ONE as i64) * 80) / 100) as i32,
                    hand: self.side,
                    progress_q16: Q16_ONE,
                    velocity_q16: index_vy,
                });
            }
        }
        None
    }

    /// Run all gesture classifiers, return highest confidence
    fn classify(&mut self, frame: u64) -> GestureResult {
        let mut best = GestureResult {
            class: GestureClass::None,
            confidence_q16: 0,
            hand: self.side,
            progress_q16: 0,
            velocity_q16: 0,
        };

        if let Some(r) = self.detect_pinch(frame) {
            if r.confidence_q16 > best.confidence_q16 {
                best = r;
            }
        }
        if let Some(r) = self.detect_grab() {
            if r.confidence_q16 > best.confidence_q16 {
                best = r;
            }
        }
        if let Some(r) = self.detect_swipe() {
            if r.confidence_q16 > best.confidence_q16 {
                best = r;
            }
        }
        if let Some(r) = self.detect_tap(frame) {
            if r.confidence_q16 > best.confidence_q16 {
                best = r;
            }
        }

        self.last_gesture = best.class;
        self.gesture_confidence_q16 = best.confidence_q16;
        best
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct Gesture3DEngine {
    left: HandSkeleton,
    right: HandSkeleton,
    history: Vec<GestureResult>,
    frame: u64,
    enabled: bool,
}

static GESTURE3D: Mutex<Option<Gesture3DEngine>> = Mutex::new(None);

impl Gesture3DEngine {
    fn new() -> Self {
        Gesture3DEngine {
            left: HandSkeleton::new(HandSide::Left),
            right: HandSkeleton::new(HandSide::Right),
            history: Vec::new(),
            frame: 0,
            enabled: true,
        }
    }

    fn update(&mut self, dt_ms: u32) {
        if !self.enabled {
            return;
        }
        self.frame = self.frame.saturating_add(1);

        let left_result = self.left.classify(self.frame);
        let right_result = self.right.classify(self.frame);

        if left_result.class != GestureClass::None {
            self.push_history(left_result);
        }
        if right_result.class != GestureClass::None {
            self.push_history(right_result);
        }

        let _ = dt_ms;
    }

    fn push_history(&mut self, result: GestureResult) {
        self.history.push(result);
        if self.history.len() > MAX_GESTURE_HISTORY {
            self.history.remove(0);
        }
    }

    fn get_gesture(&self, hand: HandSide) -> GestureResult {
        match hand {
            HandSide::Left => GestureResult {
                class: self.left.last_gesture,
                confidence_q16: self.left.gesture_confidence_q16,
                hand: HandSide::Left,
                progress_q16: 0,
                velocity_q16: 0,
            },
            HandSide::Right => GestureResult {
                class: self.right.last_gesture,
                confidence_q16: self.right.gesture_confidence_q16,
                hand: HandSide::Right,
                progress_q16: 0,
                velocity_q16: 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Integer square root via Newton's method
fn isqrt(val: u64) -> u64 {
    if val == 0 {
        return 0;
    }
    let mut x = val;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + val / x) / 2;
    }
    x
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn update_hand(side: HandSide, joints: &[Joint3D; JOINT_COUNT], dt_ms: u32) {
    let mut g = GESTURE3D.lock();
    if let Some(e) = g.as_mut() {
        match side {
            HandSide::Left => e.left.update_joints(joints, dt_ms),
            HandSide::Right => e.right.update_joints(joints, dt_ms),
        }
    }
}

pub fn process_frame(dt_ms: u32) {
    let mut g = GESTURE3D.lock();
    if let Some(e) = g.as_mut() {
        e.update(dt_ms);
    }
}

pub fn get_gesture(side: HandSide) -> GestureResult {
    let g = GESTURE3D.lock();
    g.as_ref().map_or(
        GestureResult {
            class: GestureClass::None,
            confidence_q16: 0,
            hand: side,
            progress_q16: 0,
            velocity_q16: 0,
        },
        |e| e.get_gesture(side),
    )
}

pub fn history_len() -> usize {
    let g = GESTURE3D.lock();
    g.as_ref().map_or(0, |e| e.history.len())
}

pub fn init() {
    let mut g = GESTURE3D.lock();
    *g = Some(Gesture3DEngine::new());
    serial_println!("    AR: 3D gesture recognition (pinch, grab, swipe, tap) ready");
}

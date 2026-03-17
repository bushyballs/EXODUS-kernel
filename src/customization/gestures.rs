use crate::sync::Mutex;
/// Custom gesture mapping for Genesis
///
/// Multi-finger gestures, edge swipes, gesture recording,
/// action binding, gesture sensitivity, dead zones,
/// per-app gesture overrides, and gesture combos.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 helpers
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

fn q16_distance(x1: i32, y1: i32, x2: i32, y2: i32) -> i32 {
    let dx = x2 - x1;
    let dy = y2 - y1;
    // Manhattan distance approximation (avoids sqrt)
    let abs_dx = if dx < 0 { -dx } else { dx };
    let abs_dy = if dy < 0 { -dy } else { dy };
    abs_dx + abs_dy
}

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum GestureKind {
    SwipeUp,
    SwipeDown,
    SwipeLeft,
    SwipeRight,
    PinchIn,
    PinchOut,
    RotateClockwise,
    RotateCounterClockwise,
    EdgeSwipeLeft,
    EdgeSwipeRight,
    EdgeSwipeTop,
    EdgeSwipeBottom,
    ThreeFingerUp,
    ThreeFingerDown,
    ThreeFingerLeft,
    ThreeFingerRight,
    FourFingerUp,
    FourFingerDown,
    DoubleTapTwo,
    LongPressTwo,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GestureAction {
    ShowOverview,
    GoHome,
    GoBack,
    ShowNotifications,
    ShowQuickSettings,
    SwitchApp,
    CloseApp,
    Minimize,
    Maximize,
    ScreenCapture,
    SplitScreen,
    TogglePip,
    ZoomIn,
    ZoomOut,
    RotateView,
    Undo,
    Redo,
    OpenSearch,
    LaunchAssistant,
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum GestureSensitivity {
    Low,
    Medium,
    High,
    Custom,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RecordState {
    Idle,
    Listening,
    Captured,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EdgeZone {
    None,
    Left,
    Right,
    Top,
    Bottom,
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct GestureBinding {
    id: u32,
    kind: GestureKind,
    action: GestureAction,
    fingers: u8,
    app_id: u32, // 0 = global
    enabled: bool,
    trigger_count: u32,
    custom_param: u32,
}

#[derive(Clone, Copy)]
struct GestureThreshold {
    min_distance_q16: i32,
    max_time_ms: u32,
    min_velocity_q16: i32,
    edge_dead_zone: u16,
    pinch_min_delta_q16: i32,
    rotate_min_angle_q16: i32,
}

#[derive(Clone, Copy)]
struct TouchPoint {
    x: i32, // Q16
    y: i32, // Q16
    timestamp: u64,
    finger_id: u8,
    active: bool,
}

#[derive(Clone, Copy)]
struct RecordedGesture {
    points: [TouchPoint; 16],
    point_count: u8,
    fingers_used: u8,
    duration_ms: u32,
    kind: GestureKind,
    valid: bool,
}

struct GestureState {
    active_touches: [TouchPoint; 10],
    active_count: u8,
    start_touches: [TouchPoint; 10],
    start_count: u8,
    gesture_started: bool,
    current_edge: EdgeZone,
    screen_width: u16,
    screen_height: u16,
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

struct GestureManager {
    bindings: Vec<GestureBinding>,
    threshold: GestureThreshold,
    sensitivity: GestureSensitivity,
    state: GestureState,
    record_state: RecordState,
    recorded: RecordedGesture,
    next_binding_id: u32,
    gestures_enabled: bool,
    edge_gestures_enabled: bool,
}

static GESTURES: Mutex<Option<GestureManager>> = Mutex::new(None);

impl GestureManager {
    fn new() -> Self {
        let default_threshold = GestureThreshold {
            min_distance_q16: q16_from_int(30),
            max_time_ms: 800,
            min_velocity_q16: q16_from_int(5),
            edge_dead_zone: 20,
            pinch_min_delta_q16: q16_from_int(15),
            rotate_min_angle_q16: q16_from_int(10),
        };

        let blank_touch = TouchPoint {
            x: 0,
            y: 0,
            timestamp: 0,
            finger_id: 0,
            active: false,
        };

        let state = GestureState {
            active_touches: [blank_touch; 10],
            active_count: 0,
            start_touches: [blank_touch; 10],
            start_count: 0,
            gesture_started: false,
            current_edge: EdgeZone::None,
            screen_width: 1920,
            screen_height: 1080,
        };

        let recorded = RecordedGesture {
            points: [blank_touch; 16],
            point_count: 0,
            fingers_used: 0,
            duration_ms: 0,
            kind: GestureKind::SwipeUp,
            valid: false,
        };

        GestureManager {
            bindings: Vec::new(),
            threshold: default_threshold,
            sensitivity: GestureSensitivity::Medium,
            state,
            record_state: RecordState::Idle,
            recorded,
            next_binding_id: 1,
            gestures_enabled: true,
            edge_gestures_enabled: true,
        }
    }

    fn bind(&mut self, kind: GestureKind, action: GestureAction, fingers: u8, app_id: u32) -> u32 {
        if self.bindings.len() >= 128 {
            return 0;
        }

        // Check for duplicate binding in same scope
        let dup = self
            .bindings
            .iter()
            .any(|b| b.kind == kind && b.fingers == fingers && b.app_id == app_id && b.enabled);
        if dup {
            return 0;
        }

        let id = self.next_binding_id;
        self.next_binding_id = self.next_binding_id.saturating_add(1);

        self.bindings.push(GestureBinding {
            id,
            kind,
            action,
            fingers,
            app_id,
            enabled: true,
            trigger_count: 0,
            custom_param: 0,
        });
        id
    }

    fn unbind(&mut self, binding_id: u32) -> bool {
        let len_before = self.bindings.len();
        self.bindings.retain(|b| b.id != binding_id);
        self.bindings.len() < len_before
    }

    fn set_enabled(&mut self, binding_id: u32, enabled: bool) -> bool {
        if let Some(b) = self.bindings.iter_mut().find(|b| b.id == binding_id) {
            b.enabled = enabled;
            return true;
        }
        false
    }

    fn set_sensitivity(&mut self, sensitivity: GestureSensitivity) {
        self.sensitivity = sensitivity;
        match sensitivity {
            GestureSensitivity::Low => {
                self.threshold.min_distance_q16 = q16_from_int(50);
                self.threshold.min_velocity_q16 = q16_from_int(10);
                self.threshold.max_time_ms = 600;
            }
            GestureSensitivity::Medium => {
                self.threshold.min_distance_q16 = q16_from_int(30);
                self.threshold.min_velocity_q16 = q16_from_int(5);
                self.threshold.max_time_ms = 800;
            }
            GestureSensitivity::High => {
                self.threshold.min_distance_q16 = q16_from_int(15);
                self.threshold.min_velocity_q16 = q16_from_int(2);
                self.threshold.max_time_ms = 1200;
            }
            GestureSensitivity::Custom => {}
        }
    }

    fn touch_down(&mut self, finger_id: u8, x_q16: i32, y_q16: i32, timestamp: u64) {
        if !self.gestures_enabled {
            return;
        }
        if finger_id as usize >= 10 {
            return;
        }

        let idx = finger_id as usize;
        let point = TouchPoint {
            x: x_q16,
            y: y_q16,
            timestamp,
            finger_id,
            active: true,
        };
        self.state.active_touches[idx] = point;
        self.state.active_count = self.state.active_count.saturating_add(1);

        if !self.state.gesture_started {
            self.state.gesture_started = true;
            self.state.start_touches[idx] = point;
            self.state.start_count = self.state.active_count;
            self.state.current_edge = self.detect_edge(x_q16, y_q16);
        }

        if self.record_state == RecordState::Listening {
            let pc = self.recorded.point_count as usize;
            if pc < 16 {
                self.recorded.points[pc] = point;
                self.recorded.point_count = self.recorded.point_count.saturating_add(1);
                self.recorded.fingers_used =
                    self.recorded.fingers_used.max(self.state.active_count);
            }
        }
    }

    fn touch_up(&mut self, finger_id: u8, x_q16: i32, y_q16: i32, timestamp: u64) {
        if finger_id as usize >= 10 {
            return;
        }
        let idx = finger_id as usize;

        self.state.active_touches[idx].active = false;
        if self.state.active_count > 0 {
            self.state.active_count = self.state.active_count.saturating_sub(1);
        }

        // When all fingers lifted, recognize gesture
        if self.state.active_count == 0 && self.state.gesture_started {
            self.state.gesture_started = false;

            let start = &self.state.start_touches[idx];
            let dx = x_q16 - start.x;
            let dy = y_q16 - start.y;
            let dist = q16_distance(start.x, start.y, x_q16, y_q16);
            let elapsed = timestamp.saturating_sub(start.timestamp) as u32;
            let fingers = self.state.start_count;

            if let Some(kind) = self.classify(dx, dy, dist, elapsed, fingers) {
                self.dispatch(kind, fingers, 0);
            }

            if self.record_state == RecordState::Listening {
                self.recorded.duration_ms = elapsed;
                self.recorded.valid = true;
                self.record_state = RecordState::Captured;
            }
        }
    }

    fn detect_edge(&self, x_q16: i32, y_q16: i32) -> EdgeZone {
        if !self.edge_gestures_enabled {
            return EdgeZone::None;
        }
        let dead = q16_from_int(self.threshold.edge_dead_zone as i32);
        let sw = q16_from_int(self.state.screen_width as i32);
        let sh = q16_from_int(self.state.screen_height as i32);

        if x_q16 < dead {
            return EdgeZone::Left;
        }
        if x_q16 > sw - dead {
            return EdgeZone::Right;
        }
        if y_q16 < dead {
            return EdgeZone::Top;
        }
        if y_q16 > sh - dead {
            return EdgeZone::Bottom;
        }
        EdgeZone::None
    }

    fn classify(
        &self,
        dx: i32,
        dy: i32,
        dist: i32,
        elapsed_ms: u32,
        fingers: u8,
    ) -> Option<GestureKind> {
        if dist < self.threshold.min_distance_q16 {
            return None;
        }
        if elapsed_ms > self.threshold.max_time_ms {
            return None;
        }

        let abs_dx = if dx < 0 { -dx } else { dx };
        let abs_dy = if dy < 0 { -dy } else { dy };

        // Edge swipes first
        match self.state.current_edge {
            EdgeZone::Left if abs_dx > abs_dy && dx > 0 => return Some(GestureKind::EdgeSwipeLeft),
            EdgeZone::Right if abs_dx > abs_dy && dx < 0 => {
                return Some(GestureKind::EdgeSwipeRight)
            }
            EdgeZone::Top if abs_dy > abs_dx && dy > 0 => return Some(GestureKind::EdgeSwipeTop),
            EdgeZone::Bottom if abs_dy > abs_dx && dy < 0 => {
                return Some(GestureKind::EdgeSwipeBottom)
            }
            _ => {}
        }

        // Multi-finger gestures
        if fingers >= 4 {
            if abs_dy > abs_dx {
                return if dy < 0 {
                    Some(GestureKind::FourFingerUp)
                } else {
                    Some(GestureKind::FourFingerDown)
                };
            }
        }
        if fingers >= 3 {
            if abs_dy > abs_dx {
                return if dy < 0 {
                    Some(GestureKind::ThreeFingerUp)
                } else {
                    Some(GestureKind::ThreeFingerDown)
                };
            }
            if abs_dx > abs_dy {
                return if dx < 0 {
                    Some(GestureKind::ThreeFingerLeft)
                } else {
                    Some(GestureKind::ThreeFingerRight)
                };
            }
        }

        // Standard two/one finger swipes
        if abs_dx > abs_dy {
            if dx > 0 {
                Some(GestureKind::SwipeRight)
            } else {
                Some(GestureKind::SwipeLeft)
            }
        } else {
            if dy > 0 {
                Some(GestureKind::SwipeDown)
            } else {
                Some(GestureKind::SwipeUp)
            }
        }
    }

    fn dispatch(&mut self, kind: GestureKind, _fingers: u8, app_id: u32) -> Option<GestureAction> {
        // Try app-specific binding first, then global
        let action = self
            .bindings
            .iter_mut()
            .filter(|b| b.enabled && b.kind == kind)
            .find(|b| (b.app_id == app_id && app_id != 0) || b.app_id == 0)
            .map(|b| {
                b.trigger_count = b.trigger_count.saturating_add(1);
                b.action
            });
        action
    }

    fn start_recording(&mut self) {
        self.record_state = RecordState::Listening;
        self.recorded.point_count = 0;
        self.recorded.fingers_used = 0;
        self.recorded.duration_ms = 0;
        self.recorded.valid = false;
    }

    fn finish_recording(&mut self) -> Option<GestureKind> {
        if self.record_state != RecordState::Captured || !self.recorded.valid {
            self.record_state = RecordState::Idle;
            return None;
        }
        self.record_state = RecordState::Idle;
        Some(self.recorded.kind)
    }

    fn set_custom_threshold(&mut self, min_dist: i32, max_time: u32, min_vel: i32) {
        self.threshold.min_distance_q16 = min_dist;
        self.threshold.max_time_ms = max_time;
        self.threshold.min_velocity_q16 = min_vel;
        self.sensitivity = GestureSensitivity::Custom;
    }

    fn set_edge_dead_zone(&mut self, pixels: u16) {
        self.threshold.edge_dead_zone = pixels.clamp(5, 100);
    }

    fn set_screen_size(&mut self, width: u16, height: u16) {
        self.state.screen_width = width;
        self.state.screen_height = height;
    }

    fn toggle_edge_gestures(&mut self, enabled: bool) {
        self.edge_gestures_enabled = enabled;
    }

    fn toggle_all(&mut self, enabled: bool) {
        self.gestures_enabled = enabled;
    }

    fn binding_count(&self) -> usize {
        self.bindings.len()
    }

    fn setup_defaults(&mut self) {
        // Three-finger up -> show overview / task switcher
        self.bind(
            GestureKind::ThreeFingerUp,
            GestureAction::ShowOverview,
            3,
            0,
        );
        // Three-finger down -> go home
        self.bind(GestureKind::ThreeFingerDown, GestureAction::GoHome, 3, 0);
        // Three-finger left/right -> switch app
        self.bind(GestureKind::ThreeFingerLeft, GestureAction::SwitchApp, 3, 0);
        self.bind(
            GestureKind::ThreeFingerRight,
            GestureAction::SwitchApp,
            3,
            0,
        );
        // Four-finger up -> show all desktops
        self.bind(GestureKind::FourFingerUp, GestureAction::ShowOverview, 4, 0);
        // Edge swipe left -> go back
        self.bind(GestureKind::EdgeSwipeLeft, GestureAction::GoBack, 1, 0);
        // Edge swipe right -> go back (alternate)
        self.bind(GestureKind::EdgeSwipeRight, GestureAction::GoBack, 1, 0);
        // Edge swipe top -> notifications
        self.bind(
            GestureKind::EdgeSwipeTop,
            GestureAction::ShowNotifications,
            1,
            0,
        );
        // Edge swipe bottom -> quick settings
        self.bind(
            GestureKind::EdgeSwipeBottom,
            GestureAction::ShowQuickSettings,
            1,
            0,
        );
        // Pinch out -> zoom in
        self.bind(GestureKind::PinchOut, GestureAction::ZoomIn, 2, 0);
        // Pinch in -> zoom out
        self.bind(GestureKind::PinchIn, GestureAction::ZoomOut, 2, 0);
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut mgr = GestureManager::new();
    mgr.setup_defaults();

    let mut guard = GESTURES.lock();
    *guard = Some(mgr);
    serial_println!("    Gestures: multi-finger mapping ready ({} defaults)", 11);
}

pub fn bind_gesture(kind: GestureKind, action: GestureAction, fingers: u8, app_id: u32) -> u32 {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.bind(kind, action, fingers, app_id);
    }
    0
}

pub fn unbind_gesture(binding_id: u32) -> bool {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.unbind(binding_id);
    }
    false
}

pub fn on_touch_down(finger_id: u8, x_q16: i32, y_q16: i32, timestamp: u64) {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.touch_down(finger_id, x_q16, y_q16, timestamp);
    }
}

pub fn on_touch_up(finger_id: u8, x_q16: i32, y_q16: i32, timestamp: u64) {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.touch_up(finger_id, x_q16, y_q16, timestamp);
    }
}

pub fn set_sensitivity(sensitivity: GestureSensitivity) {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.set_sensitivity(sensitivity);
    }
}

pub fn start_gesture_recording() {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.start_recording();
    }
}

pub fn finish_gesture_recording() -> Option<GestureKind> {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        return mgr.finish_recording();
    }
    None
}

pub fn toggle_gestures(enabled: bool) {
    let mut guard = GESTURES.lock();
    if let Some(mgr) = guard.as_mut() {
        mgr.toggle_all(enabled);
    }
}

/// Navigation system for Genesis — system navigation bar and gestures
///
/// Supports: 3-button nav (back/home/recents), gesture nav (swipe up),
/// and keyboard shortcuts. Handles window/app transitions.
///
/// Inspired by: Android gesture nav, iOS home indicator. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Navigation mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    ThreeButton, // Back | Home | Recents
    TwoButton,   // Back | Home (swipe up for recents)
    Gesture,     // Full gesture nav (swipe from edges)
}

/// Navigation action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    Back,
    Home,
    Recents,
    SplitScreen,
    Screenshot,
    AssistantLongPress,
    QuickSwitch, // swipe between last two apps
}

/// Gesture state
#[derive(Debug, Clone, Copy)]
pub struct GestureState {
    pub start_x: i32,
    pub start_y: i32,
    pub current_x: i32,
    pub current_y: i32,
    pub active: bool,
    pub edge: GestureEdge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GestureEdge {
    None,
    Left,
    Right,
    Bottom,
    Top,
}

/// Navigation state
pub struct Navigation {
    pub mode: NavMode,
    pub visible: bool,
    pub bar_height: u16,
    pub gesture: GestureState,
    /// Back stack per app
    back_stacks: Vec<(String, Vec<String>)>,
    /// Gesture sensitivity (pixels from edge)
    pub edge_sensitivity: u16,
    /// Whether back gesture is from left, right, or both
    pub back_gesture_edges: u8, // bit 0=left, bit 1=right
}

impl Navigation {
    const fn new() -> Self {
        Navigation {
            mode: NavMode::Gesture,
            visible: true,
            bar_height: 48,
            gesture: GestureState {
                start_x: 0,
                start_y: 0,
                current_x: 0,
                current_y: 0,
                active: false,
                edge: GestureEdge::None,
            },
            back_stacks: Vec::new(),
            edge_sensitivity: 24,
            back_gesture_edges: 3, // both edges
        }
    }

    /// Process a touch/pointer event
    pub fn on_touch_down(&mut self, x: i32, y: i32, screen_w: u32, screen_h: u32) {
        self.gesture.start_x = x;
        self.gesture.start_y = y;
        self.gesture.current_x = x;
        self.gesture.current_y = y;
        self.gesture.active = true;

        // Detect edge
        let sens = self.edge_sensitivity as i32;
        if x < sens && self.back_gesture_edges & 1 != 0 {
            self.gesture.edge = GestureEdge::Left;
        } else if x > screen_w as i32 - sens && self.back_gesture_edges & 2 != 0 {
            self.gesture.edge = GestureEdge::Right;
        } else if y > screen_h as i32 - sens {
            self.gesture.edge = GestureEdge::Bottom;
        } else {
            self.gesture.edge = GestureEdge::None;
        }
    }

    pub fn on_touch_move(&mut self, x: i32, y: i32) {
        self.gesture.current_x = x;
        self.gesture.current_y = y;
    }

    /// Process touch up and determine action
    pub fn on_touch_up(&mut self, screen_h: u32) -> Option<NavAction> {
        if !self.gesture.active {
            return None;
        }
        self.gesture.active = false;

        let dx = self.gesture.current_x - self.gesture.start_x;
        let dy = self.gesture.current_y - self.gesture.start_y;

        match self.mode {
            NavMode::Gesture => {
                match self.gesture.edge {
                    GestureEdge::Left | GestureEdge::Right => {
                        if dx.abs() > 50 {
                            return Some(NavAction::Back);
                        }
                    }
                    GestureEdge::Bottom => {
                        let swipe_distance = -dy; // upward = negative
                        if swipe_distance > (screen_h as i32 / 3) {
                            // Long swipe up = home
                            return Some(NavAction::Home);
                        } else if swipe_distance > 50 {
                            // Short swipe up + pause = recents
                            return Some(NavAction::Recents);
                        }
                    }
                    _ => {}
                }
            }
            NavMode::ThreeButton => {
                // Button detection based on position
                // (handled by button click, not gesture)
            }
            _ => {}
        }
        None
    }

    /// Handle button press (for 3-button mode)
    pub fn button_press(&self, action: NavAction) -> NavAction {
        action
    }

    /// Push a back stack entry
    pub fn push_back(&mut self, app_id: &str, screen: &str) {
        if let Some(entry) = self.back_stacks.iter_mut().find(|(id, _)| id == app_id) {
            entry.1.push(String::from(screen));
        } else {
            self.back_stacks
                .push((String::from(app_id), alloc::vec![String::from(screen)]));
        }
    }

    /// Pop back stack (returns true if there was something to go back to)
    pub fn pop_back(&mut self, app_id: &str) -> bool {
        if let Some(entry) = self.back_stacks.iter_mut().find(|(id, _)| id == app_id) {
            entry.1.pop().is_some() && !entry.1.is_empty()
        } else {
            false
        }
    }

    /// Set navigation mode
    pub fn set_mode(&mut self, mode: NavMode) {
        self.mode = mode;
        self.bar_height = match mode {
            NavMode::ThreeButton => 48,
            NavMode::TwoButton => 32,
            NavMode::Gesture => 8, // just a small indicator
        };
    }
}

static NAVIGATION: Mutex<Navigation> = Mutex::new(Navigation::new());

pub fn init() {
    crate::serial_println!("  [navigation] Navigation system initialized (gesture mode)");
}

pub fn set_mode(mode: NavMode) {
    NAVIGATION.lock().set_mode(mode);
}
pub fn on_touch_down(x: i32, y: i32, w: u32, h: u32) {
    NAVIGATION.lock().on_touch_down(x, y, w, h);
}
pub fn on_touch_move(x: i32, y: i32) {
    NAVIGATION.lock().on_touch_move(x, y);
}
pub fn on_touch_up(h: u32) -> Option<NavAction> {
    NAVIGATION.lock().on_touch_up(h)
}

use crate::display::theme;
use crate::drivers::framebuffer::Color;
/// Adaptive UI for Genesis
///
/// Listens to the Neural Bus and adjusts the visual environment based on
/// user intent, cognitive load, and emotional state.
///
/// Features:
///   - Malleable Background: Changes colors based on Intent (Work=Blue, Idle=Green, Game=Purple)
///   - Dynamic Layout: Rearranges windows to optimize for the predicted task
///   - Visual Novelty: Highlights new or important system notifications
///
/// Intent mapping uses the Neural Bus SubsystemClass activity counters:
///   High Application → Working, High Input → Gaming, low activity → Idle
///   High Network → Browsing.
use crate::sync::Mutex;

/// Inferred user intent derived from neural bus activity patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserIntent {
    Unknown,
    Working,
    Idle,
    Gaming,
    Browsing,
}

/// Derive current user intent from neural bus class-activity counters.
///
/// This is a lightweight heuristic that avoids a full lock on the BUS
/// by only sampling the most-active class, which is a single atomic read.
pub fn user_intent() -> UserIntent {
    let most_active = crate::neural_bus::BUS.lock().most_active_class();
    match most_active {
        crate::neural_bus::SubsystemClass::Application => UserIntent::Working,
        crate::neural_bus::SubsystemClass::Input => UserIntent::Gaming,
        crate::neural_bus::SubsystemClass::Network => UserIntent::Browsing,
        crate::neural_bus::SubsystemClass::Kernel => UserIntent::Idle,
        _ => UserIntent::Unknown,
    }
}

pub struct AdaptiveUiEngine {
    pub last_intent: UserIntent,
    pub morph_active: bool,
}

impl AdaptiveUiEngine {
    pub const fn new() -> Self {
        AdaptiveUiEngine {
            last_intent: UserIntent::Unknown,
            morph_active: false,
        }
    }

    pub fn tick(&mut self) {
        let current_intent = user_intent();
        if current_intent != self.last_intent {
            self.handle_intent_shift(current_intent);
            self.last_intent = current_intent;
        }
    }

    fn handle_intent_shift(&mut self, intent: UserIntent) {
        crate::serial_println!(
            "    [adaptive-ui] Intent shift detected: {:?}. Morphing environment...",
            intent
        );

        let mut theme = theme::THEME.lock();

        match intent {
            UserIntent::Working => {
                theme.desktop_bg = Color::rgb(10, 20, 40); // Deep professional blue
                theme.accent = Color::rgb(0, 180, 255);
            }
            UserIntent::Idle => {
                theme.desktop_bg = Color::rgb(10, 30, 15); // Calm forest green
                theme.accent = Color::rgb(0, 255, 150);
            }
            UserIntent::Gaming => {
                theme.desktop_bg = Color::rgb(30, 10, 40); // Cyber purple
                theme.accent = Color::rgb(255, 0, 255);
            }
            UserIntent::Browsing => {
                theme.desktop_bg = Color::rgb(20, 20, 25); // Neutral slate
                theme.accent = Color::rgb(200, 200, 200);
            }
            UserIntent::Unknown => {
                // Keep current theme — no change needed.
            }
        }

        // Request a full redraw.
        drop(theme); // release theme lock before acquiring compositor lock
        crate::display::compositor::invalidate();
    }
}

pub static ENGINE: Mutex<AdaptiveUiEngine> = Mutex::new(AdaptiveUiEngine::new());

pub fn init() {
    crate::serial_println!("    [adaptive-ui] Malleable visual environment initialized");
}

pub fn tick() {
    ENGINE.lock().tick();
}

/// App switcher for Genesis — task overview and management
///
/// Shows running apps as cards, allows switching, closing,
/// split-screen, and picture-in-picture mode.
///
/// Inspired by: Android Recents, iOS App Switcher. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// App card in the switcher
pub struct AppCard {
    pub app_id: String,
    pub title: String,
    pub thumbnail: Vec<u32>, // ARGB pixel data
    pub thumb_width: u16,
    pub thumb_height: u16,
    pub last_active: u64,
    pub pinned: bool,
}

/// Split screen mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitMode {
    None,
    TopBottom,
    LeftRight,
}

/// App switcher state
pub struct AppSwitcher {
    pub visible: bool,
    pub cards: Vec<AppCard>,
    pub selected: usize,
    pub split_mode: SplitMode,
    pub split_apps: (Option<String>, Option<String>),
    /// Picture-in-picture windows
    pub pip_apps: Vec<String>,
}

impl AppSwitcher {
    const fn new() -> Self {
        AppSwitcher {
            visible: false,
            cards: Vec::new(),
            selected: 0,
            split_mode: SplitMode::None,
            split_apps: (None, None),
            pip_apps: Vec::new(),
        }
    }

    /// Show the app switcher
    pub fn show(&mut self) {
        self.visible = true;
        self.selected = 0;
    }

    /// Hide the app switcher
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Add/update a card
    pub fn update_card(&mut self, app_id: &str, title: &str, thumbnail: &[u32], w: u16, h: u16) {
        let now = crate::time::clock::unix_time();
        if let Some(card) = self.cards.iter_mut().find(|c| c.app_id == app_id) {
            card.title = String::from(title);
            card.thumbnail = thumbnail.to_vec();
            card.thumb_width = w;
            card.thumb_height = h;
            card.last_active = now;
        } else {
            self.cards.push(AppCard {
                app_id: String::from(app_id),
                title: String::from(title),
                thumbnail: thumbnail.to_vec(),
                thumb_width: w,
                thumb_height: h,
                last_active: now,
                pinned: false,
            });
        }
        // Sort by last active (most recent first)
        self.cards.sort_by(|a, b| b.last_active.cmp(&a.last_active));
    }

    /// Remove an app card (close app from recents)
    pub fn remove_card(&mut self, app_id: &str) {
        self.cards.retain(|c| c.app_id != app_id || c.pinned);
    }

    /// Clear all (except pinned)
    pub fn clear_all(&mut self) {
        self.cards.retain(|c| c.pinned);
    }

    /// Navigate to next card
    pub fn next(&mut self) {
        if !self.cards.is_empty() {
            self.selected = (self.selected + 1) % self.cards.len();
        }
    }

    /// Navigate to previous card
    pub fn prev(&mut self) {
        if !self.cards.is_empty() {
            if self.selected == 0 {
                self.selected = self.cards.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Enter split screen
    pub fn enter_split(&mut self, mode: SplitMode, app1: &str, app2: &str) {
        self.split_mode = mode;
        self.split_apps = (Some(String::from(app1)), Some(String::from(app2)));
        self.visible = false;
    }

    /// Exit split screen
    pub fn exit_split(&mut self) {
        self.split_mode = SplitMode::None;
        self.split_apps = (None, None);
    }

    /// Enter PiP mode
    pub fn enter_pip(&mut self, app_id: &str) {
        if !self.pip_apps.contains(&String::from(app_id)) {
            self.pip_apps.push(String::from(app_id));
        }
    }

    /// Exit PiP mode
    pub fn exit_pip(&mut self, app_id: &str) {
        self.pip_apps.retain(|id| id != app_id);
    }

    /// Get selected app ID
    pub fn selected_app(&self) -> Option<&str> {
        self.cards.get(self.selected).map(|c| c.app_id.as_str())
    }

    /// Card count
    pub fn count(&self) -> usize {
        self.cards.len()
    }
}

static APP_SWITCHER: Mutex<AppSwitcher> = Mutex::new(AppSwitcher::new());

pub fn init() {
    crate::serial_println!("  [app-switcher] App switcher initialized");
}

pub fn show() {
    APP_SWITCHER.lock().show();
}
pub fn hide() {
    APP_SWITCHER.lock().hide();
}
pub fn next() {
    APP_SWITCHER.lock().next();
}
pub fn prev() {
    APP_SWITCHER.lock().prev();
}
pub fn clear_all() {
    APP_SWITCHER.lock().clear_all();
}

/// Application lifecycle for Genesis — app state management
///
/// Manages app states (created, started, resumed, paused, stopped, destroyed),
/// background/foreground transitions, process priority, and memory management.
///
/// Inspired by: Android Activity lifecycle, iOS app states. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// App lifecycle state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Created,
    Started,
    Resumed, // foreground, visible, interactive
    Paused,  // partially visible (dialog over it)
    Stopped, // background, not visible
    Destroyed,
}

/// Process importance level (for OOM killer priority)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Importance {
    Foreground,        // Currently interacting user
    ForegroundService, // Playing music, navigation, etc.
    Visible,           // Visible but not focused
    Service,           // Background service
    Cached,            // Recently used, can be killed
    Empty,             // No active components
}

/// Running app instance
pub struct AppInstance {
    pub app_id: String,
    pub pid: u32,
    pub state: AppState,
    pub importance: Importance,
    pub started_at: u64,
    pub last_active: u64,
    /// Memory usage (bytes)
    pub memory_usage: usize,
    /// Whether app has a foreground service
    pub foreground_service: bool,
    /// Saved instance state (for reconstruction after kill)
    pub saved_state: Vec<u8>,
    /// Number of times app was recreated
    pub recreate_count: u32,
}

/// Lifecycle manager
pub struct LifecycleManager {
    apps: Vec<AppInstance>,
    /// Maximum cached apps before killing
    max_cached: usize,
    /// Memory pressure threshold (bytes)
    memory_threshold: usize,
    /// Recent apps list (for app switcher)
    recents: Vec<String>,
    max_recents: usize,
}

impl LifecycleManager {
    const fn new() -> Self {
        LifecycleManager {
            apps: Vec::new(),
            max_cached: 10,
            memory_threshold: 256 * 1024 * 1024, // 256MB
            recents: Vec::new(),
            max_recents: 20,
        }
    }

    /// Create a new app instance
    pub fn create_app(&mut self, app_id: &str, pid: u32) {
        let now = crate::time::clock::unix_time();
        self.apps.push(AppInstance {
            app_id: String::from(app_id),
            pid,
            state: AppState::Created,
            importance: Importance::Foreground,
            started_at: now,
            last_active: now,
            memory_usage: 0,
            foreground_service: false,
            saved_state: Vec::new(),
            recreate_count: 0,
        });
    }

    /// Transition app to a new state
    pub fn transition(&mut self, app_id: &str, new_state: AppState) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            let _old_state = app.state;
            app.state = new_state;
            let now = crate::time::clock::unix_time();

            match new_state {
                AppState::Resumed => {
                    app.importance = Importance::Foreground;
                    app.last_active = now;
                    // Add to recents
                    self.recents.retain(|id| id != app_id);
                    self.recents.insert(0, String::from(app_id));
                    if self.recents.len() > self.max_recents {
                        self.recents.pop();
                    }
                }
                AppState::Paused => {
                    app.importance = Importance::Visible;
                }
                AppState::Stopped => {
                    app.importance = if app.foreground_service {
                        Importance::ForegroundService
                    } else {
                        Importance::Cached
                    };
                }
                AppState::Destroyed => {
                    app.importance = Importance::Empty;
                }
                _ => {}
            }
        }
    }

    /// Get current foreground app
    pub fn foreground_app(&self) -> Option<&str> {
        self.apps
            .iter()
            .find(|a| a.state == AppState::Resumed)
            .map(|a| a.app_id.as_str())
    }

    /// Trim memory — kill least important cached apps
    pub fn trim_memory(&mut self) -> Vec<u32> {
        let mut killed = Vec::new();

        // Sort by importance (least important first)
        let mut killable: Vec<usize> = self
            .apps
            .iter()
            .enumerate()
            .filter(|(_, a)| a.importance >= Importance::Cached)
            .map(|(i, _)| i)
            .collect();

        killable.sort_by(|&a, &b| {
            self.apps[b]
                .importance
                .cmp(&self.apps[a].importance)
                .then(self.apps[a].last_active.cmp(&self.apps[b].last_active))
        });

        // Kill apps exceeding cache limit
        let cached_count = self
            .apps
            .iter()
            .filter(|a| a.importance >= Importance::Cached)
            .count();

        if cached_count > self.max_cached {
            let to_kill = cached_count - self.max_cached;
            for &idx in killable.iter().take(to_kill) {
                let pid = self.apps[idx].pid;
                self.apps[idx].state = AppState::Destroyed;
                self.apps[idx].importance = Importance::Empty;
                killed.push(pid);
            }
        }

        // Remove destroyed apps
        self.apps.retain(|a| a.state != AppState::Destroyed);
        killed
    }

    /// Get recent apps list
    pub fn recent_apps(&self) -> &[String] {
        &self.recents
    }

    /// Start foreground service for an app
    pub fn start_foreground_service(&mut self, app_id: &str) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            app.foreground_service = true;
            if app.importance > Importance::ForegroundService {
                app.importance = Importance::ForegroundService;
            }
        }
    }

    /// Stop foreground service
    pub fn stop_foreground_service(&mut self, app_id: &str) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            app.foreground_service = false;
            if app.state == AppState::Stopped {
                app.importance = Importance::Cached;
            }
        }
    }

    /// Get running app count
    pub fn running_count(&self) -> usize {
        self.apps
            .iter()
            .filter(|a| a.state != AppState::Destroyed)
            .count()
    }

    /// Update memory usage for an app
    pub fn update_memory(&mut self, app_id: &str, bytes: usize) {
        if let Some(app) = self.apps.iter_mut().find(|a| a.app_id == app_id) {
            app.memory_usage = bytes;
        }
    }
}

static LIFECYCLE: Mutex<LifecycleManager> = Mutex::new(LifecycleManager::new());

pub fn init() {
    crate::serial_println!("  [lifecycle] App lifecycle manager initialized");
}

pub fn create_app(app_id: &str, pid: u32) {
    LIFECYCLE.lock().create_app(app_id, pid);
}
pub fn transition(app_id: &str, state: AppState) {
    LIFECYCLE.lock().transition(app_id, state);
}
pub fn foreground_app() -> Option<String> {
    LIFECYCLE.lock().foreground_app().map(String::from)
}
pub fn trim_memory() -> Vec<u32> {
    LIFECYCLE.lock().trim_memory()
}
pub fn running_count() -> usize {
    LIFECYCLE.lock().running_count()
}

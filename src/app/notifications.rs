/// Notification system for Genesis — app and system notifications
///
/// Manages notifications with priorities, channels, actions, and
/// a notification shade. Apps post notifications through the API;
/// the system displays and manages them.
///
/// Inspired by: Android NotificationManager, macOS UNUserNotificationCenter. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Notification priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Low,
    Default,
    High,
    Urgent,
}

/// Notification category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Message,
    Email,
    Call,
    Alarm,
    Reminder,
    Social,
    Progress,
    System,
    Error,
}

/// Action button on a notification
#[derive(Clone)]
pub struct NotifAction {
    pub label: String,
    pub action_id: u32,
    pub destructive: bool,
}

/// A notification
pub struct Notification {
    pub id: u32,
    pub app_id: String,
    pub title: String,
    pub body: String,
    pub category: Category,
    pub priority: Priority,
    pub actions: Vec<NotifAction>,
    pub timestamp: u64,
    pub read: bool,
    pub persistent: bool,
    /// Progress (0-100, for Progress category)
    pub progress: Option<u8>,
    /// Group key (notifications with same key are grouped)
    pub group: Option<String>,
    /// Icon name
    pub icon: Option<String>,
}

/// Notification channel (app-defined category with user-configurable settings)
pub struct NotifChannel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub priority: Priority,
    pub sound: bool,
    pub vibrate: bool,
    pub show_badge: bool,
    pub enabled: bool,
}

/// Do-not-disturb mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndMode {
    Off,
    Priority, // Only urgent and alarms
    AlarmsOnly,
    TotalSilence,
}

/// Notification manager state
pub struct NotifManager {
    notifications: Vec<Notification>,
    channels: Vec<NotifChannel>,
    next_id: u32,
    dnd_mode: DndMode,
    /// Maximum notifications per app
    max_per_app: usize,
    /// Total notification count
    pub total_posted: u64,
    pub total_dismissed: u64,
}

impl NotifManager {
    const fn new() -> Self {
        NotifManager {
            notifications: Vec::new(),
            channels: Vec::new(),
            next_id: 1,
            dnd_mode: DndMode::Off,
            max_per_app: 50,
            total_posted: 0,
            total_dismissed: 0,
        }
    }

    /// Post a new notification
    pub fn post(
        &mut self,
        app_id: &str,
        title: &str,
        body: &str,
        category: Category,
        priority: Priority,
    ) -> u32 {
        // Check DND
        if self.should_suppress(priority) {
            return 0;
        }

        // Enforce per-app limit
        let app_count = self
            .notifications
            .iter()
            .filter(|n| n.app_id == app_id)
            .count();
        if app_count >= self.max_per_app {
            // Remove oldest from this app
            if let Some(pos) = self.notifications.iter().position(|n| n.app_id == app_id) {
                self.notifications.remove(pos);
            }
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        self.notifications.push(Notification {
            id,
            app_id: String::from(app_id),
            title: String::from(title),
            body: String::from(body),
            category,
            priority,
            actions: Vec::new(),
            timestamp: crate::time::clock::unix_time(),
            read: false,
            persistent: false,
            progress: None,
            group: None,
            icon: None,
        });

        self.total_posted = self.total_posted.saturating_add(1);
        id
    }

    /// Dismiss a notification
    pub fn dismiss(&mut self, id: u32) -> bool {
        if let Some(pos) = self.notifications.iter().position(|n| n.id == id) {
            if !self.notifications[pos].persistent {
                self.notifications.remove(pos);
                self.total_dismissed = self.total_dismissed.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Mark as read
    pub fn mark_read(&mut self, id: u32) {
        if let Some(notif) = self.notifications.iter_mut().find(|n| n.id == id) {
            notif.read = true;
        }
    }

    /// Dismiss all for an app
    pub fn dismiss_all(&mut self, app_id: &str) {
        self.notifications
            .retain(|n| n.app_id != app_id || n.persistent);
    }

    /// Get unread count
    pub fn unread_count(&self) -> usize {
        self.notifications.iter().filter(|n| !n.read).count()
    }

    /// Get all notifications (newest first)
    pub fn list(&self) -> Vec<(u32, String, String, bool)> {
        let mut result: Vec<_> = self
            .notifications
            .iter()
            .map(|n| (n.id, n.title.clone(), n.body.clone(), n.read))
            .collect();
        result.reverse();
        result
    }

    /// Set DND mode
    pub fn set_dnd(&mut self, mode: DndMode) {
        self.dnd_mode = mode;
    }

    fn should_suppress(&self, priority: Priority) -> bool {
        match self.dnd_mode {
            DndMode::Off => false,
            DndMode::Priority => priority < Priority::High,
            DndMode::AlarmsOnly => priority < Priority::Urgent,
            DndMode::TotalSilence => true,
        }
    }

    /// Update progress on a notification
    pub fn update_progress(&mut self, id: u32, progress: u8) {
        if let Some(notif) = self.notifications.iter_mut().find(|n| n.id == id) {
            notif.progress = Some(progress.min(100));
        }
    }
}

static NOTIF_MANAGER: Mutex<NotifManager> = Mutex::new(NotifManager::new());

pub fn init() {
    crate::serial_println!("  [notifications] Notification system initialized");
}

pub fn post(app_id: &str, title: &str, body: &str) -> u32 {
    NOTIF_MANAGER
        .lock()
        .post(app_id, title, body, Category::System, Priority::Default)
}

pub fn dismiss(id: u32) -> bool {
    NOTIF_MANAGER.lock().dismiss(id)
}
pub fn unread_count() -> usize {
    NOTIF_MANAGER.lock().unread_count()
}
pub fn set_dnd(mode: DndMode) {
    NOTIF_MANAGER.lock().set_dnd(mode);
}

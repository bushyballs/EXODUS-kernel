use crate::sync::Mutex;
/// Notification pull-down UI
///
/// Part of the Genesis System UI. Renders the swipe-down
/// notification shade with grouped alerts and actions.
use alloc::string::String;
use alloc::vec::Vec;

/// A single notification entry
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub timestamp: u64,
    pub dismissed: bool,
}

pub struct NotificationShade {
    pub notifications: Vec<Notification>,
    pub visible: bool,
    next_id: u64,
}

impl NotificationShade {
    pub fn new() -> Self {
        NotificationShade {
            notifications: Vec::new(),
            visible: false,
            next_id: 1,
        }
    }

    /// Show the notification shade (pull down).
    pub fn open(&mut self) {
        self.visible = true;
        crate::serial_println!(
            "  [notify] shade opened ({} notifications)",
            self.active_count()
        );
    }

    /// Hide the notification shade (swipe up / close).
    pub fn close(&mut self) {
        self.visible = false;
        crate::serial_println!("  [notify] shade closed");
    }

    /// Post a new notification and return its ID.
    pub fn post(&mut self, title: &str, body: &str, timestamp: u64) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        crate::serial_println!("  [notify] post #{}: {}", id, title);

        self.notifications.push(Notification {
            id,
            title: String::from(title),
            body: String::from(body),
            timestamp,
            dismissed: false,
        });

        id
    }

    /// Dismiss a notification by ID.
    ///
    /// Marks it as dismissed; it will be cleaned up on next purge.
    pub fn dismiss(&mut self, id: u64) {
        for notif in self.notifications.iter_mut() {
            if notif.id == id && !notif.dismissed {
                notif.dismissed = true;
                crate::serial_println!("  [notify] dismissed #{}", id);
                return;
            }
        }
    }

    /// Dismiss all notifications.
    pub fn dismiss_all(&mut self) {
        for notif in self.notifications.iter_mut() {
            notif.dismissed = true;
        }
        crate::serial_println!("  [notify] dismissed all");
    }

    /// Remove all dismissed notifications from memory.
    pub fn purge_dismissed(&mut self) {
        let before = self.notifications.len();
        self.notifications.retain(|n| !n.dismissed);
        let removed = before - self.notifications.len();
        if removed > 0 {
            crate::serial_println!("  [notify] purged {} dismissed notifications", removed);
        }
    }

    /// Number of active (non-dismissed) notifications.
    pub fn active_count(&self) -> usize {
        self.notifications.iter().filter(|n| !n.dismissed).count()
    }

    /// Get a notification by ID.
    pub fn get(&self, id: u64) -> Option<&Notification> {
        self.notifications.iter().find(|n| n.id == id)
    }

    /// Check if the shade is currently visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }
}

static NOTIFICATION_SHADE: Mutex<Option<NotificationShade>> = Mutex::new(None);

pub fn init() {
    *NOTIFICATION_SHADE.lock() = Some(NotificationShade::new());
    crate::serial_println!("  [notify] Notification shade initialized");
}

/// Post a global notification.
pub fn post(title: &str, body: &str, timestamp: u64) -> u64 {
    match NOTIFICATION_SHADE.lock().as_mut() {
        Some(shade) => shade.post(title, body, timestamp),
        None => 0,
    }
}

/// Dismiss a global notification by ID.
pub fn dismiss(id: u64) {
    if let Some(ref mut shade) = *NOTIFICATION_SHADE.lock() {
        shade.dismiss(id);
    }
}

/// Open the global notification shade.
pub fn open() {
    if let Some(ref mut shade) = *NOTIFICATION_SHADE.lock() {
        shade.open();
    }
}

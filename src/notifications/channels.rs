use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Notification priority levels
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationPriority {
    Critical,
    High,
    Default,
    Low,
    Min,
}

/// A notification channel for grouping similar notifications
#[derive(Clone, Copy, Debug)]
pub struct NotificationChannel {
    pub id: u32,
    pub name_hash: u32,
    pub priority: NotificationPriority,
    pub sound_enabled: bool,
    pub vibrate: bool,
    pub show_badge: bool,
    pub show_on_lock: bool,
    pub do_not_disturb_override: bool,
}

impl NotificationChannel {
    pub fn new(id: u32, name_hash: u32, priority: NotificationPriority) -> Self {
        Self {
            id,
            name_hash,
            priority,
            sound_enabled: true,
            vibrate: true,
            show_badge: true,
            show_on_lock: true,
            do_not_disturb_override: matches!(priority, NotificationPriority::Critical),
        }
    }
}

/// A notification instance
#[derive(Clone, Copy, Debug)]
pub struct Notification {
    pub id: u32,
    pub channel_id: u32,
    pub title_hash: u32,
    pub body_hash: u32,
    pub timestamp: u64,
    pub read: bool,
    pub app_id: u32,
    pub action_count: u8,
    pub group_key: u32,
}

impl Notification {
    pub fn new(
        id: u32,
        channel_id: u32,
        title_hash: u32,
        body_hash: u32,
        timestamp: u64,
        app_id: u32,
    ) -> Self {
        Self {
            id,
            channel_id,
            title_hash,
            body_hash,
            timestamp,
            read: false,
            app_id,
            action_count: 0,
            group_key: 0,
        }
    }
}

/// Manages notification channels and notifications
pub struct NotificationManager {
    channels: Vec<NotificationChannel>,
    notifications: Vec<Notification>,
    next_id: u32,
    total_posted: u64,
    total_dismissed: u64,
    total_auto_cleared: u64,
}

impl NotificationManager {
    pub fn new() -> Self {
        Self {
            channels: vec![],
            notifications: vec![],
            next_id: 1,
            total_posted: 0,
            total_dismissed: 0,
            total_auto_cleared: 0,
        }
    }

    /// Create a new notification channel
    pub fn create_channel(&mut self, name_hash: u32, priority: NotificationPriority) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let channel = NotificationChannel::new(id, name_hash, priority);
        self.channels.push(channel);

        serial_println!(
            "[NOTIF] Created channel {} with priority {:?}",
            id,
            priority
        );

        id
    }

    /// Post a new notification
    pub fn post(
        &mut self,
        channel_id: u32,
        title_hash: u32,
        body_hash: u32,
        timestamp: u64,
        app_id: u32,
    ) -> Option<u32> {
        // Verify channel exists
        if !self.channels.iter().any(|c| c.id == channel_id) {
            serial_println!("[NOTIF] Error: channel {} not found", channel_id);
            return None;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        let notif = Notification::new(id, channel_id, title_hash, body_hash, timestamp, app_id);
        self.notifications.push(notif);
        self.total_posted = self.total_posted.saturating_add(1);

        serial_println!(
            "[NOTIF] Posted notification {} on channel {} from app {}",
            id,
            channel_id,
            app_id
        );

        Some(id)
    }

    /// Dismiss a specific notification
    pub fn dismiss(&mut self, id: u32) -> bool {
        if let Some(pos) = self.notifications.iter().position(|n| n.id == id) {
            self.notifications.remove(pos);
            self.total_dismissed = self.total_dismissed.saturating_add(1);
            serial_println!("[NOTIF] Dismissed notification {}", id);
            true
        } else {
            false
        }
    }

    /// Dismiss all notifications from a specific app
    pub fn dismiss_all(&mut self, app_id: u32) -> u32 {
        let initial_count = self.notifications.len();
        self.notifications.retain(|n| n.app_id != app_id);
        let dismissed = (initial_count - self.notifications.len()) as u32;
        self.total_dismissed += dismissed as u64;

        serial_println!(
            "[NOTIF] Dismissed {} notifications from app {}",
            dismissed,
            app_id
        );

        dismissed
    }

    /// Mark a notification as read
    pub fn mark_read(&mut self, id: u32) -> bool {
        if let Some(notif) = self.notifications.iter_mut().find(|n| n.id == id) {
            notif.read = true;
            serial_println!("[NOTIF] Marked notification {} as read", id);
            true
        } else {
            false
        }
    }

    /// Get all active (unread) notifications
    pub fn get_active(&self) -> Vec<Notification> {
        self.notifications
            .iter()
            .filter(|n| !n.read)
            .copied()
            .collect()
    }

    /// Get channel by ID
    pub fn get_channel(&self, id: u32) -> Option<&NotificationChannel> {
        self.channels.iter().find(|c| c.id == id)
    }

    /// Get notification count
    pub fn notification_count(&self) -> usize {
        self.notifications.len()
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (
            self.total_posted,
            self.total_dismissed,
            self.total_auto_cleared,
        )
    }
}

static NOTIF_MANAGER: Mutex<Option<NotificationManager>> = Mutex::new(None);

/// Initialize the notification manager
pub fn init() {
    let mut lock = NOTIF_MANAGER.lock();
    *lock = Some(NotificationManager::new());
    serial_println!("[NOTIF] Notification manager initialized");
}

/// Get a reference to the notification manager
pub fn get_manager() -> &'static Mutex<Option<NotificationManager>> {
    &NOTIF_MANAGER
}

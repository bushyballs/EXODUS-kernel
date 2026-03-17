use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

/// Style for displaying grouped notifications
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupStyle {
    Bundled,
    Conversation,
    MessagingStyle,
    BigText,
    BigPicture,
}

/// A group of related notifications
#[derive(Clone, Debug)]
pub struct NotificationGroup {
    pub group_key: u32,
    pub app_id: u32,
    pub style: GroupStyle,
    pub notification_ids: Vec<u32>,
    pub summary_hash: u32,
    pub auto_group: bool,
    pub child_count: u32,
}

impl NotificationGroup {
    pub fn new(group_key: u32, app_id: u32, style: GroupStyle, auto_group: bool) -> Self {
        Self {
            group_key,
            app_id,
            style,
            notification_ids: vec![],
            summary_hash: 0,
            auto_group,
            child_count: 0,
        }
    }

    pub fn add_notification(&mut self, id: u32) {
        self.notification_ids.push(id);
        self.child_count = self.notification_ids.len() as u32;
    }

    pub fn remove_notification(&mut self, id: u32) -> bool {
        if let Some(pos) = self.notification_ids.iter().position(|&nid| nid == id) {
            self.notification_ids.remove(pos);
            self.child_count = self.notification_ids.len() as u32;
            true
        } else {
            false
        }
    }
}

/// Manages notification grouping
pub struct GroupManager {
    groups: Vec<NotificationGroup>,
    auto_group_threshold: u8,
}

impl GroupManager {
    pub fn new() -> Self {
        Self {
            groups: vec![],
            auto_group_threshold: 4,
        }
    }

    /// Add a notification to an existing group
    pub fn add_to_group(&mut self, group_key: u32, notification_id: u32) -> bool {
        if let Some(group) = self.groups.iter_mut().find(|g| g.group_key == group_key) {
            group.add_notification(notification_id);
            serial_println!(
                "[GROUP] Added notification {} to group {}",
                notification_id,
                group_key
            );
            true
        } else {
            serial_println!("[GROUP] Error: group {} not found", group_key);
            false
        }
    }

    /// Create a new notification group
    pub fn create_group(
        &mut self,
        group_key: u32,
        app_id: u32,
        style: GroupStyle,
        auto_group: bool,
    ) -> bool {
        // Check if group already exists
        if self.groups.iter().any(|g| g.group_key == group_key) {
            serial_println!("[GROUP] Error: group {} already exists", group_key);
            return false;
        }

        let group = NotificationGroup::new(group_key, app_id, style, auto_group);
        self.groups.push(group);

        serial_println!(
            "[GROUP] Created group {} for app {} with style {:?}",
            group_key,
            app_id,
            style
        );

        true
    }

    /// Get summary text hash for a group
    pub fn get_summary(&self, group_key: u32) -> Option<u32> {
        self.groups
            .iter()
            .find(|g| g.group_key == group_key)
            .map(|g| g.summary_hash)
    }

    /// Expand a group to show all notifications
    pub fn expand_group(&self, group_key: u32) -> Option<Vec<u32>> {
        self.groups
            .iter()
            .find(|g| g.group_key == group_key)
            .map(|g| g.notification_ids.clone())
    }

    /// Check if notifications from an app should be auto-grouped
    pub fn auto_group_check(&mut self, app_id: u32, notification_count: u32) -> bool {
        if notification_count >= self.auto_group_threshold as u32 {
            // Generate auto-group key from app_id
            let group_key = app_id ^ 0xA0000000;

            // Create auto-group if it doesn't exist
            if !self.groups.iter().any(|g| g.group_key == group_key) {
                self.create_group(group_key, app_id, GroupStyle::Bundled, true);
                serial_println!(
                    "[GROUP] Auto-grouping {} notifications from app {}",
                    notification_count,
                    app_id
                );
            }
            true
        } else {
            false
        }
    }

    /// Remove a notification from all groups
    pub fn remove_notification(&mut self, notification_id: u32) -> bool {
        let mut found = false;
        for group in &mut self.groups {
            if group.remove_notification(notification_id) {
                found = true;
            }
        }

        // Remove empty groups
        self.groups.retain(|g| !g.notification_ids.is_empty());

        if found {
            serial_println!(
                "[GROUP] Removed notification {} from groups",
                notification_id
            );
        }

        found
    }

    /// Get group by key
    pub fn get_group(&self, group_key: u32) -> Option<&NotificationGroup> {
        self.groups.iter().find(|g| g.group_key == group_key)
    }

    /// Get all groups for an app
    pub fn get_app_groups(&self, app_id: u32) -> Vec<&NotificationGroup> {
        self.groups.iter().filter(|g| g.app_id == app_id).collect()
    }

    /// Get group count
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    /// Set auto-group threshold
    pub fn set_auto_group_threshold(&mut self, threshold: u8) {
        self.auto_group_threshold = threshold;
        serial_println!("[GROUP] Auto-group threshold set to {}", threshold);
    }
}

static GROUP_MGR: Mutex<Option<GroupManager>> = Mutex::new(None);

/// Initialize the group manager
pub fn init() {
    let mut lock = GROUP_MGR.lock();
    *lock = Some(GroupManager::new());
    serial_println!("[GROUP] Group manager initialized");
}

/// Get a reference to the group manager
pub fn get_manager() -> &'static Mutex<Option<GroupManager>> {
    &GROUP_MGR
}

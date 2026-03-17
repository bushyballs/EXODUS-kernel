// Rich notifications: images, actions, progress bars, custom layouts, media controls

use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;
use crate::{serial_print, serial_println};

/// Q16 fixed-point unit
const Q16_ONE: i32 = 65536;

/// Maximum actions per notification
const MAX_ACTIONS: usize = 5;

/// Maximum progress bar segments
const MAX_PROGRESS_SEGMENTS: usize = 8;

/// Type of rich content attached to a notification
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RichContentType {
    Image,
    LargeIcon,
    ProgressBar,
    MediaControls,
    ActionButtons,
    CustomLayout,
    InlineReply,
    BigText,
    BigPicture,
}

/// Media playback state for media-control notifications
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaPlaybackState {
    Stopped,
    Playing,
    Paused,
    Buffering,
    Error,
}

/// A single action button on a notification
#[derive(Clone, Copy, Debug)]
pub struct NotifAction {
    pub action_id: u32,
    pub label_hash: u32,
    pub icon_hash: u32,
    pub is_destructive: bool,
    pub requires_unlock: bool,
    pub is_reply: bool,
}

impl NotifAction {
    pub fn new(action_id: u32, label_hash: u32, icon_hash: u32) -> Self {
        Self {
            action_id,
            label_hash,
            icon_hash,
            is_destructive: false,
            requires_unlock: false,
            is_reply: false,
        }
    }

    pub fn with_destructive(mut self) -> Self {
        self.is_destructive = true;
        self
    }

    pub fn with_requires_unlock(mut self) -> Self {
        self.requires_unlock = true;
        self
    }

    pub fn with_reply(mut self) -> Self {
        self.is_reply = true;
        self
    }
}

/// Progress bar state for download/upload/task notifications
#[derive(Clone, Copy, Debug)]
pub struct ProgressBar {
    pub progress_q16: i32,
    pub max_q16: i32,
    pub indeterminate: bool,
    pub segments: [i32; MAX_PROGRESS_SEGMENTS],
    pub segment_count: u8,
    pub color_hash: u32,
    pub show_percentage: bool,
}

impl ProgressBar {
    pub fn new() -> Self {
        Self {
            progress_q16: 0,
            max_q16: Q16_ONE,
            indeterminate: false,
            segments: [0; MAX_PROGRESS_SEGMENTS],
            segment_count: 0,
            color_hash: 0x4CAF50FF,
            show_percentage: true,
        }
    }

    /// Set progress as Q16 fraction (0 to Q16_ONE)
    pub fn set_progress(&mut self, progress_q16: i32) {
        self.progress_q16 = progress_q16.clamp(0, self.max_q16);
        self.indeterminate = false;
    }

    /// Set progress from integer percentage (0-100)
    pub fn set_progress_percent(&mut self, percent: u8) {
        let pct = percent.min(100) as i32;
        self.progress_q16 = (((pct as i64) * (self.max_q16 as i64)) / 100) as i32;
        self.indeterminate = false;
    }

    /// Get current percentage as integer (0-100)
    pub fn get_percent(&self) -> u8 {
        if self.max_q16 == 0 {
            return 0;
        }
        let pct = (((self.progress_q16 as i64) * 100) / (self.max_q16 as i64)) as i32;
        pct.clamp(0, 100) as u8
    }

    /// Add a segment for multi-part progress bars
    pub fn add_segment(&mut self, value_q16: i32) -> bool {
        if (self.segment_count as usize) >= MAX_PROGRESS_SEGMENTS {
            return false;
        }
        self.segments[self.segment_count as usize] = value_q16;
        self.segment_count = self.segment_count.saturating_add(1);
        true
    }
}

/// Media control state for audio/video notifications
#[derive(Clone, Copy, Debug)]
pub struct MediaControls {
    pub playback_state: MediaPlaybackState,
    pub track_title_hash: u32,
    pub artist_hash: u32,
    pub album_art_hash: u32,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub has_previous: bool,
    pub has_next: bool,
    pub has_seek: bool,
    pub volume_q16: i32,
}

impl MediaControls {
    pub fn new() -> Self {
        Self {
            playback_state: MediaPlaybackState::Stopped,
            track_title_hash: 0,
            artist_hash: 0,
            album_art_hash: 0,
            position_ms: 0,
            duration_ms: 0,
            has_previous: true,
            has_next: true,
            has_seek: true,
            volume_q16: Q16_ONE / 2,
        }
    }

    /// Set the playback state
    pub fn set_state(&mut self, state: MediaPlaybackState) {
        self.playback_state = state;
    }

    /// Update playback position
    pub fn set_position(&mut self, position_ms: u64) {
        self.position_ms = position_ms.min(self.duration_ms);
    }

    /// Get playback progress as Q16 fraction
    pub fn progress_q16(&self) -> i32 {
        if self.duration_ms == 0 {
            return 0;
        }
        (((self.position_ms as i64) << 16) / (self.duration_ms as i64)) as i32
    }
}

/// Custom layout element for rich notifications
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutElement {
    Title,
    Subtitle,
    Body,
    Image,
    Icon,
    Timestamp,
    Divider,
    Spacer,
}

/// Custom layout definition
#[derive(Clone, Debug)]
pub struct CustomLayout {
    pub layout_id: u32,
    pub elements: Vec<LayoutElement>,
    pub background_color: u32,
    pub accent_color: u32,
    pub compact: bool,
}

impl CustomLayout {
    pub fn new(layout_id: u32) -> Self {
        Self {
            layout_id,
            elements: vec![
                LayoutElement::Icon,
                LayoutElement::Title,
                LayoutElement::Body,
                LayoutElement::Timestamp,
            ],
            background_color: 0xFFFFFFFF,
            accent_color: 0x2196F3FF,
            compact: false,
        }
    }

    /// Add an element to the layout
    pub fn add_element(&mut self, element: LayoutElement) {
        self.elements.push(element);
    }

    /// Remove all instances of an element type
    pub fn remove_element(&mut self, element: LayoutElement) {
        self.elements.retain(|e| *e != element);
    }
}

/// A rich notification combining content, actions, progress, and media
#[derive(Clone, Debug)]
pub struct RichNotification {
    pub notification_id: u32,
    pub content_type: RichContentType,
    pub image_hash: u32,
    pub large_icon_hash: u32,
    pub actions: Vec<NotifAction>,
    pub progress: Option<ProgressBar>,
    pub media: Option<MediaControls>,
    pub layout: Option<CustomLayout>,
    pub expanded: bool,
    pub created_at: u64,
}

impl RichNotification {
    pub fn new(notification_id: u32, content_type: RichContentType, created_at: u64) -> Self {
        Self {
            notification_id,
            content_type,
            image_hash: 0,
            large_icon_hash: 0,
            actions: vec![],
            progress: None,
            media: None,
            layout: None,
            expanded: false,
            created_at,
        }
    }

    /// Add an action button
    pub fn add_action(&mut self, action: NotifAction) -> bool {
        if self.actions.len() >= MAX_ACTIONS {
            serial_println!(
                "[RICH-NOTIF] Max actions reached for notification {}",
                self.notification_id
            );
            return false;
        }
        self.actions.push(action);
        true
    }

    /// Set a progress bar on this notification
    pub fn set_progress(&mut self, progress: ProgressBar) {
        self.progress = Some(progress);
        self.content_type = RichContentType::ProgressBar;
    }

    /// Set media controls on this notification
    pub fn set_media(&mut self, media: MediaControls) {
        self.media = Some(media);
        self.content_type = RichContentType::MediaControls;
    }

    /// Set a custom layout
    pub fn set_layout(&mut self, layout: CustomLayout) {
        self.layout = Some(layout);
        self.content_type = RichContentType::CustomLayout;
    }

    /// Toggle expanded/collapsed state
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }
}

/// Manages all rich notifications in the system
pub struct RichNotifManager {
    notifications: Vec<RichNotification>,
    templates: Vec<CustomLayout>,
    next_layout_id: u32,
    total_created: u64,
    total_actions_invoked: u64,
    total_progress_updates: u64,
}

impl RichNotifManager {
    pub fn new() -> Self {
        Self {
            notifications: vec![],
            templates: vec![],
            next_layout_id: 1,
            total_created: 0,
            total_actions_invoked: 0,
            total_progress_updates: 0,
        }
    }

    /// Create a rich notification
    pub fn create(
        &mut self,
        notification_id: u32,
        content_type: RichContentType,
        timestamp: u64,
    ) -> &mut RichNotification {
        let notif = RichNotification::new(notification_id, content_type, timestamp);
        self.notifications.push(notif);
        self.total_created = self.total_created.saturating_add(1);

        serial_println!(
            "[RICH-NOTIF] Created {:?} notification for {}",
            content_type,
            notification_id
        );

        self.notifications.last_mut().unwrap()
    }

    /// Create a progress notification
    pub fn create_progress(
        &mut self,
        notification_id: u32,
        timestamp: u64,
        indeterminate: bool,
    ) -> u32 {
        let notif = self.create(notification_id, RichContentType::ProgressBar, timestamp);
        let mut pb = ProgressBar::new();
        pb.indeterminate = indeterminate;
        notif.progress = Some(pb);
        notification_id
    }

    /// Update progress on an existing notification
    pub fn update_progress(&mut self, notification_id: u32, percent: u8) -> bool {
        if let Some(notif) = self.notifications.iter_mut()
            .find(|n| n.notification_id == notification_id)
        {
            if let Some(ref mut pb) = notif.progress {
                pb.set_progress_percent(percent);
                self.total_progress_updates = self.total_progress_updates.saturating_add(1);
                return true;
            }
        }
        false
    }

    /// Create a media control notification
    pub fn create_media(
        &mut self,
        notification_id: u32,
        track_title_hash: u32,
        artist_hash: u32,
        duration_ms: u64,
        timestamp: u64,
    ) -> u32 {
        let notif = self.create(notification_id, RichContentType::MediaControls, timestamp);
        let mut mc = MediaControls::new();
        mc.track_title_hash = track_title_hash;
        mc.artist_hash = artist_hash;
        mc.duration_ms = duration_ms;
        notif.media = Some(mc);
        notification_id
    }

    /// Invoke an action on a notification
    pub fn invoke_action(&mut self, notification_id: u32, action_id: u32) -> bool {
        if let Some(notif) = self.notifications.iter()
            .find(|n| n.notification_id == notification_id)
        {
            if notif.actions.iter().any(|a| a.action_id == action_id) {
                self.total_actions_invoked = self.total_actions_invoked.saturating_add(1);
                serial_println!(
                    "[RICH-NOTIF] Action {} invoked on notification {}",
                    action_id,
                    notification_id
                );
                return true;
            }
        }
        false
    }

    /// Register a reusable layout template
    pub fn register_template(&mut self, layout: CustomLayout) -> u32 {
        let id = self.next_layout_id;
        self.next_layout_id = self.next_layout_id.saturating_add(1);
        let mut template = layout;
        template.layout_id = id;
        self.templates.push(template);

        serial_println!("[RICH-NOTIF] Registered layout template {}", id);
        id
    }

    /// Apply a template to a notification
    pub fn apply_template(&mut self, notification_id: u32, template_id: u32) -> bool {
        let template = self.templates.iter()
            .find(|t| t.layout_id == template_id)
            .cloned();

        if let Some(tmpl) = template {
            if let Some(notif) = self.notifications.iter_mut()
                .find(|n| n.notification_id == notification_id)
            {
                notif.set_layout(tmpl);
                serial_println!(
                    "[RICH-NOTIF] Applied template {} to notification {}",
                    template_id,
                    notification_id
                );
                return true;
            }
        }
        false
    }

    /// Remove a rich notification
    pub fn remove(&mut self, notification_id: u32) -> bool {
        if let Some(pos) = self.notifications.iter()
            .position(|n| n.notification_id == notification_id)
        {
            self.notifications.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get a rich notification by ID
    pub fn get(&self, notification_id: u32) -> Option<&RichNotification> {
        self.notifications.iter().find(|n| n.notification_id == notification_id)
    }

    /// Get count of rich notifications
    pub fn count(&self) -> usize {
        self.notifications.len()
    }

    /// Get stats: (created, actions_invoked, progress_updates)
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total_created, self.total_actions_invoked, self.total_progress_updates)
    }
}

static RICH_NOTIF_MGR: Mutex<Option<RichNotifManager>> = Mutex::new(None);

/// Initialize the rich notification manager
pub fn init() {
    let mut lock = RICH_NOTIF_MGR.lock();
    *lock = Some(RichNotifManager::new());
    serial_println!("[RICH-NOTIF] Rich notification manager initialized");
}

/// Get a reference to the rich notification manager
pub fn get_manager() -> &'static Mutex<Option<RichNotifManager>> {
    &RICH_NOTIF_MGR
}

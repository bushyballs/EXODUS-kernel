/// AI-powered cross-device sync for Genesis
///
/// Smart sync prioritization, conflict resolution,
/// bandwidth-aware transfer, contextual handoff prediction.
///
/// Inspired by: Apple Continuity Intelligence, Google Nearby. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Sync priority from AI
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncPriority {
    Immediate,  // Active edits, clipboard, call handoff
    High,       // Notifications, messages, active tabs
    Normal,     // Contacts, calendar, bookmarks
    Low,        // Photos, media, backups
    Background, // App data, settings, training data
}

/// Handoff prediction
pub struct HandoffPrediction {
    pub from_device: String,
    pub to_device: String,
    pub activity: String,
    pub probability: f32,
    pub trigger: String,
}

/// Conflict resolution strategy
pub struct ConflictResolution {
    pub item: String,
    pub strategy: ResolutionStrategy,
    pub confidence: f32,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStrategy {
    UseNewest,
    UseDevice,
    Merge,
    AskUser,
    KeepBoth,
}

/// AI sync engine
pub struct AiSyncEngine {
    pub enabled: bool,
    pub device_patterns: Vec<DeviceUsagePattern>,
    pub sync_history: Vec<SyncEvent>,
    pub handoff_predictions: Vec<HandoffPrediction>,
    pub bandwidth_aware: bool,
    pub battery_aware: bool,
    pub total_syncs: u64,
    pub total_conflicts_resolved: u64,
}

pub struct DeviceUsagePattern {
    pub device_name: String,
    pub device_type: String,
    pub usage_hours: Vec<(u8, u8)>, // (start_hour, end_hour)
    pub primary_activities: Vec<String>,
}

pub struct SyncEvent {
    pub item_type: String,
    pub direction: String,
    pub bytes: u64,
    pub timestamp: u64,
    pub success: bool,
}

impl AiSyncEngine {
    const fn new() -> Self {
        AiSyncEngine {
            enabled: true,
            device_patterns: Vec::new(),
            sync_history: Vec::new(),
            handoff_predictions: Vec::new(),
            bandwidth_aware: true,
            battery_aware: true,
            total_syncs: 0,
            total_conflicts_resolved: 0,
        }
    }

    /// Prioritize sync items based on context
    pub fn prioritize(&self, item_type: &str, is_active_edit: bool) -> SyncPriority {
        if is_active_edit {
            return SyncPriority::Immediate;
        }
        match item_type {
            "clipboard" | "handoff" | "call" => SyncPriority::Immediate,
            "notification" | "message" | "tab" => SyncPriority::High,
            "contact" | "calendar" | "bookmark" => SyncPriority::Normal,
            "photo" | "media" | "backup" => SyncPriority::Low,
            _ => SyncPriority::Background,
        }
    }

    /// Resolve a sync conflict with AI
    pub fn resolve_conflict(
        &mut self,
        item: &str,
        local_time: u64,
        remote_time: u64,
        item_type: &str,
    ) -> ConflictResolution {
        self.total_conflicts_resolved = self.total_conflicts_resolved.saturating_add(1);
        let time_diff = if local_time > remote_time {
            local_time - remote_time
        } else {
            remote_time - local_time
        };

        let (strategy, reason) = if time_diff < 5 {
            // Nearly simultaneous edits
            (
                ResolutionStrategy::Merge,
                "Nearly simultaneous edits — merging both",
            )
        } else if item_type == "contact" || item_type == "calendar" {
            // Structured data — use newest
            (
                ResolutionStrategy::UseNewest,
                "Structured data — using most recent version",
            )
        } else if item_type == "document" || item_type == "note" {
            if time_diff > 3600 {
                (
                    ResolutionStrategy::KeepBoth,
                    "Long time gap — keeping both versions",
                )
            } else {
                (ResolutionStrategy::UseNewest, "Recent edit — using newest")
            }
        } else {
            (
                ResolutionStrategy::UseNewest,
                "Default — using most recent version",
            )
        };

        ConflictResolution {
            item: String::from(item),
            strategy,
            confidence: if time_diff > 60 { 0.9 } else { 0.6 },
            reason: String::from(reason),
        }
    }

    /// Predict device handoff
    pub fn predict_handoff(&self) -> Vec<HandoffPrediction> {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let mut predictions = Vec::new();

        for pattern in &self.device_patterns {
            for (start, _end) in &pattern.usage_hours {
                if hour == *start {
                    predictions.push(HandoffPrediction {
                        from_device: String::from("current"),
                        to_device: pattern.device_name.clone(),
                        activity: pattern
                            .primary_activities
                            .first()
                            .cloned()
                            .unwrap_or_else(|| String::from("general")),
                        probability: 0.7,
                        trigger: alloc::format!(
                            "Time-based: {} is typically used at {}:00",
                            pattern.device_name,
                            hour
                        ),
                    });
                }
            }
        }

        predictions
    }

    /// Should sync happen now given battery/bandwidth?
    pub fn should_sync_now(
        &self,
        priority: SyncPriority,
        battery: u8,
        on_wifi: bool,
        size_kb: u64,
    ) -> bool {
        match priority {
            SyncPriority::Immediate => true,
            SyncPriority::High => battery > 10,
            SyncPriority::Normal => battery > 20 && (on_wifi || size_kb < 100),
            SyncPriority::Low => battery > 30 && on_wifi,
            SyncPriority::Background => battery > 50 && on_wifi,
        }
    }

    /// Record a sync event
    pub fn record_sync(&mut self, item_type: &str, direction: &str, bytes: u64, success: bool) {
        self.total_syncs = self.total_syncs.saturating_add(1);
        self.sync_history.push(SyncEvent {
            item_type: String::from(item_type),
            direction: String::from(direction),
            bytes,
            timestamp: crate::time::clock::unix_time(),
            success,
        });
        if self.sync_history.len() > 1000 {
            self.sync_history.remove(0);
        }
    }
}

static AI_SYNC: Mutex<AiSyncEngine> = Mutex::new(AiSyncEngine::new());

pub fn init() {
    crate::serial_println!(
        "    [ai-sync] AI cross-device sync initialized (priority, conflict, handoff)"
    );
}

pub fn prioritize(item_type: &str, active: bool) -> SyncPriority {
    AI_SYNC.lock().prioritize(item_type, active)
}

pub fn resolve_conflict(
    item: &str,
    local_t: u64,
    remote_t: u64,
    itype: &str,
) -> ConflictResolution {
    AI_SYNC
        .lock()
        .resolve_conflict(item, local_t, remote_t, itype)
}

pub fn should_sync_now(pri: SyncPriority, battery: u8, wifi: bool, size: u64) -> bool {
    AI_SYNC.lock().should_sync_now(pri, battery, wifi, size)
}

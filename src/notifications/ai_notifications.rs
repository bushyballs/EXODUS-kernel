use crate::sync::Mutex;
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

use super::channels::NotificationPriority;

/// AI-predicted importance level
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportanceLevel {
    Urgent,
    Important,
    Routine,
    LowPriority,
    Spam,
}

/// AI-generated insights for a notification
#[derive(Clone, Copy, Debug)]
pub struct NotifInsight {
    pub notification_id: u32,
    pub predicted_importance: ImportanceLevel,
    pub suggested_action: u32,       // Hash of suggested action
    pub optimal_delivery_time: u64,  // Timestamp
    pub context_relevance_score: u8, // 0-100
}

impl NotifInsight {
    pub fn new(notification_id: u32) -> Self {
        Self {
            notification_id,
            predicted_importance: ImportanceLevel::Routine,
            suggested_action: 0,
            optimal_delivery_time: 0,
            context_relevance_score: 50,
        }
    }
}

/// AI-powered notification intelligence engine
pub struct AiNotificationEngine {
    insights: Vec<NotifInsight>,
    user_interaction_history: [u32; 24], // Interactions per hour
    spam_pattern_count: u32,
    smart_reply_enabled: bool,
}

impl AiNotificationEngine {
    pub fn new() -> Self {
        Self {
            insights: vec![],
            user_interaction_history: [0; 24],
            spam_pattern_count: 0,
            smart_reply_enabled: true,
        }
    }

    /// Predict importance based on time-of-day, app, and priority
    pub fn predict_importance(
        &mut self,
        notification_id: u32,
        app_id: u32,
        priority: NotificationPriority,
        timestamp: u64,
    ) -> ImportanceLevel {
        let hour = ((timestamp / 3600) % 24) as usize;

        // Calculate base score from priority
        let mut score: i32 = match priority {
            NotificationPriority::Critical => 90,
            NotificationPriority::High => 70,
            NotificationPriority::Default => 50,
            NotificationPriority::Low => 30,
            NotificationPriority::Min => 10,
        };

        // Adjust based on user activity patterns
        let hour_activity = self.user_interaction_history[hour];
        if hour_activity > 10 {
            // User is active during this hour, boost importance
            score += 10;
        } else if hour_activity == 0 {
            // User is typically inactive, reduce importance
            score -= 15;
        }

        // Penalize apps that spam
        if self.is_app_spamming(app_id) {
            score -= 30;
        }

        // Determine importance level
        let importance = if score >= 85 {
            ImportanceLevel::Urgent
        } else if score >= 65 {
            ImportanceLevel::Important
        } else if score >= 40 {
            ImportanceLevel::Routine
        } else if score >= 20 {
            ImportanceLevel::LowPriority
        } else {
            ImportanceLevel::Spam
        };

        // Create insight
        let mut insight = NotifInsight::new(notification_id);
        insight.predicted_importance = importance;
        insight.context_relevance_score = score.clamp(0, 100) as u8;
        insight.optimal_delivery_time = self.calculate_optimal_time(timestamp, importance);
        self.insights.push(insight);

        serial_println!(
            "[AI-NOTIF] Predicted importance {:?} (score: {}) for notification {}",
            importance,
            score,
            notification_id
        );

        importance
    }

    /// Suggest smart replies for a notification
    pub fn suggest_smart_replies(&self, _notification_id: u32) -> Vec<u32> {
        if !self.smart_reply_enabled {
            return vec![];
        }

        // Return hashes of common smart replies
        // In a real implementation, these would be generated based on notification content
        vec![
            0x1A2B3C4D, // "OK"
            0x2B3C4D5E, // "Thanks"
            0x3C4D5E6F, // "On my way"
        ]
    }

    /// Calculate optimal time to deliver or bundle notifications
    pub fn optimal_bundle_time(&self, current_time: u64) -> u64 {
        let hour = ((current_time / 3600) % 24) as usize;

        // Find the next hour with high user activity
        for i in 1..24 {
            let next_hour = (hour + i) % 24;
            if self.user_interaction_history[next_hour] > 5 {
                return current_time + (i as u64 * 3600);
            }
        }

        // Default to next hour
        current_time + 3600
    }

    /// Detect if an app is spamming notifications
    pub fn detect_notification_spam(&mut self, app_id: u32, count_last_hour: u32) -> bool {
        if count_last_hour > 20 {
            self.spam_pattern_count = self.spam_pattern_count.saturating_add(1);
            serial_println!(
                "[AI-NOTIF] Spam detected from app {}: {} notifications in last hour",
                app_id,
                count_last_hour
            );
            true
        } else {
            false
        }
    }

    /// Record user interaction
    pub fn record_interaction(&mut self, timestamp: u64) {
        let hour = ((timestamp / 3600) % 24) as usize;
        self.user_interaction_history[hour] = self.user_interaction_history[hour].saturating_add(1);
    }

    /// Get insight for a notification
    pub fn get_insight(&self, notification_id: u32) -> Option<&NotifInsight> {
        self.insights
            .iter()
            .find(|i| i.notification_id == notification_id)
    }

    /// Check if app is currently spamming
    fn is_app_spamming(&self, _app_id: u32) -> bool {
        // Simplified check - in real implementation would track per-app
        self.spam_pattern_count > 0
    }

    /// Calculate optimal delivery time
    fn calculate_optimal_time(&self, current_time: u64, importance: ImportanceLevel) -> u64 {
        match importance {
            ImportanceLevel::Urgent | ImportanceLevel::Important => {
                // Deliver immediately
                current_time
            }
            ImportanceLevel::Routine => {
                // Bundle and deliver at next active hour
                self.optimal_bundle_time(current_time)
            }
            ImportanceLevel::LowPriority | ImportanceLevel::Spam => {
                // Delay significantly
                current_time + (4 * 3600) // 4 hours later
            }
        }
    }

    /// Enable/disable smart replies
    pub fn set_smart_reply(&mut self, enabled: bool) {
        self.smart_reply_enabled = enabled;
        serial_println!(
            "[AI-NOTIF] Smart replies {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Get spam pattern count
    pub fn spam_count(&self) -> u32 {
        self.spam_pattern_count
    }

    /// Get total insights
    pub fn insight_count(&self) -> usize {
        self.insights.len()
    }

    /// Clear old insights
    pub fn clear_old_insights(&mut self, before_timestamp: u64) {
        let initial_count = self.insights.len();
        self.insights
            .retain(|i| i.optimal_delivery_time >= before_timestamp);
        let removed = initial_count - self.insights.len();
        if removed > 0 {
            serial_println!("[AI-NOTIF] Cleared {} old insights", removed);
        }
    }
}

static AI_NOTIF: Mutex<Option<AiNotificationEngine>> = Mutex::new(None);

/// Initialize the AI notification engine
pub fn init() {
    let mut lock = AI_NOTIF.lock();
    *lock = Some(AiNotificationEngine::new());
    serial_println!("[AI-NOTIF] AI notification engine initialized");
}

/// Get a reference to the AI notification engine
pub fn get_engine() -> &'static Mutex<Option<AiNotificationEngine>> {
    &AI_NOTIF
}

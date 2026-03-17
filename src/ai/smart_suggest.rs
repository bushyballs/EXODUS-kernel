/// Smart suggestions for Genesis
///
/// Predictive text, app suggestions, action suggestions,
/// contextual awareness, and adaptive learning.
///
/// Uses Q16 fixed-point arithmetic throughout for scoring,
/// with temporal decay, sequence pattern recognition, and
/// contextual boosting based on current activity.
///
/// Inspired by: Android Adaptive, Apple Siri Suggestions. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 65536;
const Q16_ZERO: i32 = 0;
const Q16_HALF: i32 = 32768;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_from_int(x: i32) -> i32 {
    x << 16
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Suggestion type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionType {
    App,
    Contact,
    Action,
    Text,
    Search,
    Setting,
    Shortcut,
    Reply,
}

/// A suggestion with Q16 confidence scoring
pub struct Suggestion {
    pub suggestion_type: SuggestionType,
    pub title: String,
    pub subtitle: String,
    pub confidence: i32, // Q16 score
    pub action_uri: String,
    pub icon: String,
}

/// Context signal for suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSignal {
    TimeOfDay,
    DayOfWeek,
    Location,
    RecentApp,
    Notification,
    CalendarEvent,
    BatteryLevel,
    Connectivity,
}

/// Usage pattern with Q16 frequency and decay
pub struct UsagePattern {
    pub app_id: String,
    pub hour: u8,
    pub day_of_week: u8,
    pub frequency: u32,
    pub last_used: u64,
    /// Decayed score computed during suggestion (Q16)
    pub decayed_score: i32,
}

/// Smart reply template
pub struct SmartReply {
    pub trigger_pattern: String,
    pub replies: Vec<String>,
}

/// Sequence pattern: app A followed by app B within a time window
pub struct SequencePattern {
    pub from_app: String,
    pub to_app: String,
    /// How many times this sequence has been observed
    pub count: u32,
    /// Average time gap in seconds between the two apps
    pub avg_gap_secs: u64,
}

/// Current context state that influences suggestions
pub struct ContextState {
    pub current_app: String,
    pub hour: u8,
    pub day_of_week: u8,
    pub battery_level: u8,
    pub is_charging: bool,
    pub wifi_connected: bool,
    pub headphones_connected: bool,
    pub notifications_pending: u32,
    pub last_interaction_time: u64,
}

impl ContextState {
    const fn new() -> Self {
        ContextState {
            current_app: String::new(),
            hour: 0,
            day_of_week: 0,
            battery_level: 100,
            is_charging: false,
            wifi_connected: false,
            headphones_connected: false,
            notifications_pending: 0,
            last_interaction_time: 0,
        }
    }

    fn update_time(&mut self) {
        let now = crate::time::clock::unix_time();
        self.hour = ((now / 3600) % 24) as u8;
        self.day_of_week = ((now / 86400) % 7) as u8;
        self.last_interaction_time = now;
    }
}

// ---------------------------------------------------------------------------
// Decay computation
// ---------------------------------------------------------------------------

/// Compute temporal decay factor (Q16) based on elapsed time.
///
/// Uses a piecewise linear approximation of exponential decay:
///   - last hour: decay = 1.0 (Q16_ONE)
///   - 1-6 hours: linear from 1.0 to 0.7
///   - 6-24 hours: linear from 0.7 to 0.4
///   - 1-7 days: linear from 0.4 to 0.15
///   - >7 days: linear from 0.15 toward 0.02 (floor)
fn temporal_decay(now: u64, last_used: u64) -> i32 {
    if last_used == 0 || last_used > now {
        return Q16_ONE / 10; // minimal default
    }
    let elapsed = now - last_used;
    let hour_secs: u64 = 3600;
    let day_secs: u64 = 86400;

    if elapsed < hour_secs {
        Q16_ONE // fresh
    } else if elapsed < 6 * hour_secs {
        // Linear 1.0 -> 0.7 over 5 hours
        let frac = ((elapsed - hour_secs) * 100 / (5 * hour_secs)) as i32; // 0..100
        Q16_ONE - q16_mul(q16_from_int(30) / 100, q16_from_int(frac) / 100)
    } else if elapsed < day_secs {
        // Linear 0.7 -> 0.4 over 18 hours
        let frac = ((elapsed - 6 * hour_secs) * 100 / (18 * hour_secs)) as i32;
        let frac = if frac > 100 { 100 } else { frac };
        let _high = q16_mul(Q16_ONE, 70 * Q16_ONE / 100 / Q16_ONE); // 0.7
        let _low = q16_mul(Q16_ONE, 40 * Q16_ONE / 100 / Q16_ONE); // 0.4
                                                                   // Simpler: direct computation
        let base = (Q16_ONE * 70) / 100; // 45875 ~ 0.7
        let drop = (Q16_ONE * 30) / 100; // 19660 ~ 0.3
        base - (drop * frac) / 100
    } else if elapsed < 7 * day_secs {
        // Linear 0.4 -> 0.15 over 6 days
        let frac = ((elapsed - day_secs) * 100 / (6 * day_secs)) as i32;
        let frac = if frac > 100 { 100 } else { frac };
        let base = (Q16_ONE * 40) / 100;
        let drop = (Q16_ONE * 25) / 100;
        base - (drop * frac) / 100
    } else {
        // >7 days: 0.15 -> 0.02
        let extra_days = (elapsed - 7 * day_secs) / day_secs;
        let frac = if extra_days > 30 {
            100
        } else {
            (extra_days * 100 / 30) as i32
        };
        let base = (Q16_ONE * 15) / 100;
        let drop = (Q16_ONE * 13) / 100;
        let result = base - (drop * frac) / 100;
        if result < (Q16_ONE * 2) / 100 {
            (Q16_ONE * 2) / 100
        } else {
            result
        }
    }
}

// ---------------------------------------------------------------------------
// Suggestion engine
// ---------------------------------------------------------------------------

/// Smart suggestion engine
pub struct SuggestionEngine {
    pub enabled: bool,
    pub patterns: Vec<UsagePattern>,
    pub smart_replies: Vec<SmartReply>,
    pub app_frequency: BTreeMap<String, u32>,
    pub recent_actions: Vec<String>,
    pub max_recent: usize,
    pub learning_enabled: bool,
    /// Sequence patterns (app A -> app B)
    pub sequences: Vec<SequencePattern>,
    /// Maximum sequence patterns to track
    pub max_sequences: usize,
    /// Current context
    pub context: ContextState,
    /// Time-of-day weight multiplier (Q16, default Q16_ONE)
    pub time_weight: i32,
    /// Frequency weight multiplier (Q16, default Q16_ONE)
    pub freq_weight: i32,
    /// Sequence weight multiplier (Q16, default Q16_ONE)
    pub seq_weight: i32,
    /// Recency weight multiplier (Q16, default Q16_ONE)
    pub recency_weight: i32,
}

impl SuggestionEngine {
    const fn new() -> Self {
        SuggestionEngine {
            enabled: true,
            patterns: Vec::new(),
            smart_replies: Vec::new(),
            app_frequency: BTreeMap::new(),
            recent_actions: Vec::new(),
            max_recent: 100,
            learning_enabled: true,
            sequences: Vec::new(),
            max_sequences: 200,
            context: ContextState::new(),
            time_weight: Q16_ONE,
            freq_weight: Q16_ONE / 4,      // 0.25
            seq_weight: (Q16_ONE * 3) / 4, // 0.75
            recency_weight: Q16_HALF,      // 0.5
        }
    }

    /// Record an app usage event, updating patterns and sequence tracking
    pub fn record_app_usage(&mut self, app_id: &str) {
        if !self.learning_enabled {
            return;
        }

        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let day = ((now / 86400) % 7) as u8;

        // Update overall frequency
        let freq = self.app_frequency.entry(String::from(app_id)).or_insert(0);
        *freq = freq.saturating_add(1);

        // Update or create time-of-day pattern
        if let Some(pattern) = self
            .patterns
            .iter_mut()
            .find(|p| p.app_id == app_id && p.hour == hour && p.day_of_week == day)
        {
            pattern.frequency = pattern.frequency.saturating_add(1);
            pattern.last_used = now;
        } else {
            self.patterns.push(UsagePattern {
                app_id: String::from(app_id),
                hour,
                day_of_week: day,
                frequency: 1,
                last_used: now,
                decayed_score: Q16_ZERO,
            });
        }

        // Update sequence patterns: track what app was used before this one
        let prev_app = self.context.current_app.clone();
        if !prev_app.is_empty() && prev_app != app_id {
            let gap = now.saturating_sub(self.context.last_interaction_time);
            // Only track sequences within 10-minute window
            if gap < 600 {
                if let Some(seq) = self
                    .sequences
                    .iter_mut()
                    .find(|s| s.from_app == prev_app && s.to_app == app_id)
                {
                    // Update running average gap
                    let total = seq.avg_gap_secs * seq.count as u64 + gap;
                    seq.count = seq.count.saturating_add(1);
                    seq.avg_gap_secs = total / seq.count as u64;
                } else {
                    if self.sequences.len() < self.max_sequences {
                        self.sequences.push(SequencePattern {
                            from_app: prev_app,
                            to_app: String::from(app_id),
                            count: 1,
                            avg_gap_secs: gap,
                        });
                    } else {
                        // Evict the lowest-count sequence
                        if let Some(min_idx) = self
                            .sequences
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, s)| s.count)
                            .map(|(i, _)| i)
                        {
                            self.sequences[min_idx] = SequencePattern {
                                from_app: self.context.current_app.clone(),
                                to_app: String::from(app_id),
                                count: 1,
                                avg_gap_secs: gap,
                            };
                        }
                    }
                }
            }
        }

        // Update context
        self.context.current_app = String::from(app_id);
        self.context.update_time();
    }

    /// Record a generic action (for action sequence suggestions)
    pub fn record_action(&mut self, action: &str) {
        if self.recent_actions.len() >= self.max_recent {
            self.recent_actions.remove(0);
        }
        self.recent_actions.push(String::from(action));
    }

    /// Update the context state from external signals
    pub fn update_context(&mut self, signal: ContextSignal, value: u32) {
        match signal {
            ContextSignal::BatteryLevel => {
                self.context.battery_level = value as u8;
            }
            ContextSignal::Connectivity => {
                self.context.wifi_connected = value != 0;
            }
            _ => {}
        }
        self.context.update_time();
    }

    /// Get app suggestions for current context with full scoring pipeline
    pub fn suggest_apps(&self, max: usize) -> Vec<Suggestion> {
        if !self.enabled {
            return Vec::new();
        }

        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        let day = ((now / 86400) % 7) as u8;

        let mut scores: BTreeMap<String, i32> = BTreeMap::new();

        // --- Factor 1: Time-of-day pattern matching ---
        for pattern in &self.patterns {
            let decay = temporal_decay(now, pattern.last_used);
            let freq_q16 = q16_from_int(pattern.frequency as i32);

            // Exact hour+day match: strongest signal
            if pattern.hour == hour && pattern.day_of_week == day {
                let contribution = q16_mul(q16_mul(freq_q16, decay), self.time_weight);
                *scores.entry(pattern.app_id.clone()).or_insert(Q16_ZERO) += contribution;
            }
            // Same hour, different day: moderate signal
            else if pattern.hour == hour {
                let contribution = q16_mul(
                    q16_mul(freq_q16, decay),
                    q16_mul(self.time_weight, Q16_ONE / 3), // 1/3 weight
                );
                *scores.entry(pattern.app_id.clone()).or_insert(Q16_ZERO) += contribution;
            }
            // Adjacent hours (+-1): weak signal
            else if pattern.hour == hour.wrapping_add(1) % 24
                || pattern.hour == hour.wrapping_sub(1) % 24
            {
                let contribution = q16_mul(
                    q16_mul(freq_q16, decay),
                    q16_mul(self.time_weight, Q16_ONE / 6), // 1/6 weight
                );
                *scores.entry(pattern.app_id.clone()).or_insert(Q16_ZERO) += contribution;
            }
        }

        // --- Factor 2: Overall frequency (with diminishing returns) ---
        for (app, freq) in &self.app_frequency {
            // Use sqrt-like diminishing returns: score = sqrt(freq) in Q16
            // Approximate: for freq N, use N * (ONE / max(1, sqrt(N)))
            let f = *freq as i32;
            let sqrt_f = q16_isqrt(f as u32);
            let contribution = if sqrt_f > 0 {
                q16_mul(q16_from_int(sqrt_f as i32), self.freq_weight)
            } else {
                q16_mul(q16_from_int(1), self.freq_weight)
            };
            *scores.entry(app.clone()).or_insert(Q16_ZERO) += contribution;
        }

        // --- Factor 3: Sequence prediction ---
        // If user just used app X, boost apps that commonly follow X
        if !self.context.current_app.is_empty() {
            let current = &self.context.current_app;
            for seq in &self.sequences {
                if seq.from_app == *current && seq.count >= 2 {
                    let seq_score = q16_mul(q16_from_int(seq.count as i32), self.seq_weight);
                    *scores.entry(seq.to_app.clone()).or_insert(Q16_ZERO) += seq_score;
                }
            }
        }

        // --- Factor 4: Recency boost ---
        // Apps used in the last few actions get a boost
        let recent_window = self.recent_actions.len().min(5);
        for i in 0..recent_window {
            let idx = self.recent_actions.len() - 1 - i;
            let action = &self.recent_actions[idx];
            // Closer actions get higher boost (linearly)
            let position_factor = q16_div(
                q16_from_int((recent_window - i) as i32),
                q16_from_int(recent_window as i32),
            );
            let boost = q16_mul(self.recency_weight, position_factor);
            *scores.entry(action.clone()).or_insert(Q16_ZERO) += boost;
        }

        // --- Factor 5: Contextual boosting ---
        // Low battery -> boost battery-related apps
        if self.context.battery_level < 20 {
            let battery_apps = ["settings", "battery", "power_save"];
            for app in &battery_apps {
                *scores.entry(String::from(*app)).or_insert(Q16_ZERO) += Q16_ONE / 2;
            }
        }
        // Headphones connected -> boost media apps
        if self.context.headphones_connected {
            let media_apps = ["music", "podcast", "video", "audio"];
            for app in &media_apps {
                *scores.entry(String::from(*app)).or_insert(Q16_ZERO) += Q16_ONE / 3;
            }
        }

        // Don't suggest the currently-active app
        if !self.context.current_app.is_empty() {
            scores.remove(&self.context.current_app);
        }

        // --- Build and sort suggestions ---
        let mut suggestions: Vec<Suggestion> = scores
            .into_iter()
            .map(|(app, score)| Suggestion {
                suggestion_type: SuggestionType::App,
                title: app.clone(),
                subtitle: String::new(),
                confidence: score,
                action_uri: format!("app://{}", app),
                icon: app,
            })
            .collect();

        suggestions.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        suggestions.truncate(max);
        suggestions
    }

    /// Generate smart replies for a message using pattern matching
    pub fn suggest_replies(&self, message: &str) -> Vec<String> {
        let lower = message.to_lowercase();
        let mut replies: Vec<(String, i32)> = Vec::new(); // (reply, score)

        // --- Question patterns ---
        if lower.ends_with('?') {
            if lower.contains("how are you") || lower.contains("how's it going") {
                replies.push((String::from("I'm good, thanks!"), Q16_ONE));
                replies.push((String::from("Doing well!"), Q16_ONE - Q16_ONE / 10));
                replies.push((String::from("Great, how about you?"), Q16_ONE - Q16_ONE / 5));
            } else if lower.contains("want to") || lower.contains("wanna") {
                replies.push((String::from("Sure!"), Q16_ONE));
                replies.push((String::from("Sounds good!"), Q16_ONE - Q16_ONE / 10));
                replies.push((String::from("Maybe later"), Q16_ONE / 2));
            } else if lower.starts_with("can you") || lower.starts_with("could you") {
                replies.push((String::from("Sure, I can do that"), Q16_ONE));
                replies.push((String::from("Of course!"), Q16_ONE - Q16_ONE / 10));
                replies.push((String::from("I'll try"), Q16_ONE / 2));
            } else if lower.starts_with("do you") || lower.starts_with("did you") {
                replies.push((String::from("Yes!"), Q16_ONE));
                replies.push((String::from("Not yet"), Q16_ONE / 2));
                replies.push((String::from("Working on it"), Q16_ONE / 3));
            } else if lower.starts_with("when") {
                replies.push((String::from("Soon!"), Q16_ONE / 2));
                replies.push((String::from("Let me check"), Q16_ONE));
                replies.push((String::from("Not sure yet"), Q16_ONE / 3));
            } else if lower.starts_with("where") {
                replies.push((String::from("Let me find out"), Q16_ONE));
                replies.push((String::from("I'll check"), Q16_ONE / 2));
            } else {
                // Generic question
                replies.push((String::from("Let me think about that"), Q16_ONE / 2));
                replies.push((String::from("Good question!"), Q16_ONE / 3));
            }
        }

        // --- Gratitude patterns ---
        if lower.contains("thank") || lower.contains("thanks") || lower.contains("thx") {
            replies.push((String::from("You're welcome!"), Q16_ONE));
            replies.push((String::from("No problem!"), Q16_ONE - Q16_ONE / 10));
            replies.push((String::from("Anytime!"), Q16_ONE / 2));
        }

        // --- Greeting patterns ---
        if lower.starts_with("hi")
            || lower.starts_with("hello")
            || lower.starts_with("hey")
            || lower.starts_with("good morning")
            || lower.starts_with("good evening")
        {
            replies.push((String::from("Hey!"), Q16_ONE));
            replies.push((String::from("Hello!"), Q16_ONE - Q16_ONE / 10));
            replies.push((String::from("Hi there!"), Q16_ONE / 2));
        }

        // --- Farewell patterns ---
        if lower.starts_with("bye")
            || lower.contains("goodbye")
            || lower.contains("see you")
            || lower.contains("gotta go")
            || lower.contains("talk later")
        {
            replies.push((String::from("See you!"), Q16_ONE));
            replies.push((String::from("Bye!"), Q16_ONE - Q16_ONE / 10));
            replies.push((String::from("Take care!"), Q16_ONE / 2));
        }

        // --- Affirmative/negative ---
        let trimmed = lower.trim();
        if trimmed == "yes"
            || trimmed == "no"
            || trimmed == "ok"
            || trimmed == "okay"
            || trimmed == "sure"
        {
            replies.push((String::from("Got it!"), Q16_ONE));
            replies.push((String::from("Understood"), Q16_ONE / 2));
        }

        // --- Apology patterns ---
        if lower.contains("sorry") || lower.contains("my bad") || lower.contains("apologize") {
            replies.push((String::from("No worries!"), Q16_ONE));
            replies.push((String::from("It's okay"), Q16_ONE / 2));
            replies.push((String::from("Don't worry about it"), Q16_ONE / 3));
        }

        // --- Agreement patterns ---
        if lower.contains("sounds good")
            || lower.contains("i agree")
            || lower.contains("makes sense")
        {
            replies.push((String::from("Great!"), Q16_ONE));
            replies.push((String::from("Perfect"), Q16_ONE / 2));
        }

        // Sort by score descending, deduplicate, take top 3
        replies.sort_by(|a, b| b.1.cmp(&a.1));
        let mut seen = Vec::new();
        let mut result = Vec::new();
        for (reply, _score) in replies {
            if !seen.contains(&reply) {
                seen.push(reply.clone());
                result.push(reply);
                if result.len() >= 3 {
                    break;
                }
            }
        }
        result
    }

    /// Get action suggestions based on current context
    pub fn suggest_actions(&self) -> Vec<Suggestion> {
        if !self.enabled {
            return Vec::new();
        }

        let mut actions: Vec<Suggestion> = Vec::new();
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;

        // --- Time-based suggestions ---
        // Morning (6-10)
        if hour >= 6 && hour < 10 {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Action,
                title: String::from("Check notifications"),
                subtitle: String::from("Good morning"),
                confidence: (Q16_ONE * 80) / 100,
                action_uri: String::from("action://notifications"),
                icon: String::from("bell"),
            });
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Action,
                title: String::from("View schedule"),
                subtitle: String::from("Plan your day"),
                confidence: (Q16_ONE * 70) / 100,
                action_uri: String::from("action://calendar"),
                icon: String::from("calendar"),
            });
        }

        // Work hours (9-17)
        if hour >= 9 && hour < 17 {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Shortcut,
                title: String::from("Open workspace"),
                subtitle: String::from("Work mode"),
                confidence: (Q16_ONE * 60) / 100,
                action_uri: String::from("shortcut://workspace"),
                icon: String::from("briefcase"),
            });
        }

        // Lunch time (11-13)
        if hour >= 11 && hour < 13 {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Action,
                title: String::from("Take a break"),
                subtitle: String::from("Lunch time"),
                confidence: (Q16_ONE * 50) / 100,
                action_uri: String::from("action://break"),
                icon: String::from("coffee"),
            });
        }

        // Evening (20+)
        if hour >= 20 {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Setting,
                title: String::from("Enable Do Not Disturb"),
                subtitle: String::from("It's getting late"),
                confidence: (Q16_ONE * 60) / 100,
                action_uri: String::from("settings://dnd"),
                icon: String::from("moon"),
            });
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Setting,
                title: String::from("Reduce brightness"),
                subtitle: String::from("Easier on the eyes"),
                confidence: (Q16_ONE * 50) / 100,
                action_uri: String::from("settings://brightness"),
                icon: String::from("sun"),
            });
        }

        // --- Battery-based suggestions ---
        if self.context.battery_level < 15 && !self.context.is_charging {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Setting,
                title: String::from("Enable power saving"),
                subtitle: format!("Battery at {}%", self.context.battery_level),
                confidence: (Q16_ONE * 90) / 100,
                action_uri: String::from("settings://power_save"),
                icon: String::from("battery_low"),
            });
        }

        // --- Notification-based suggestions ---
        if self.context.notifications_pending > 5 {
            actions.push(Suggestion {
                suggestion_type: SuggestionType::Action,
                title: String::from("Clear notifications"),
                subtitle: format!("{} pending", self.context.notifications_pending),
                confidence: (Q16_ONE * 65) / 100,
                action_uri: String::from("action://clear_notifications"),
                icon: String::from("bell_off"),
            });
        }

        // --- Sequence-based action suggestions ---
        // If user commonly follows current app with a specific action
        if !self.context.current_app.is_empty() {
            for seq in &self.sequences {
                if seq.from_app == self.context.current_app && seq.count >= 3 {
                    let confidence = q16_mul(q16_from_int(seq.count.min(20) as i32), Q16_ONE / 20);
                    actions.push(Suggestion {
                        suggestion_type: SuggestionType::Shortcut,
                        title: format!("Open {}", seq.to_app),
                        subtitle: format!("Usually next ({}x)", seq.count),
                        confidence,
                        action_uri: format!("app://{}", seq.to_app),
                        icon: seq.to_app.clone(),
                    });
                }
            }
        }

        // Sort by confidence
        actions.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        actions.truncate(6);
        actions
    }

    /// Suggest text completions based on recent input patterns
    pub fn suggest_text(&self, partial: &str, max: usize) -> Vec<Suggestion> {
        if !self.enabled || partial.is_empty() {
            return Vec::new();
        }

        let lower = partial.to_lowercase();
        let mut suggestions = Vec::new();

        // Common command completions
        let commands = [
            ("ls", "list files", "action://ls"),
            ("cd", "change directory", "action://cd"),
            ("install", "install package", "action://install"),
            ("find", "search files", "action://find"),
            ("set", "change setting", "action://set"),
            ("help", "show help", "action://help"),
            ("update", "update packages", "action://update"),
            ("delete", "delete file", "action://delete"),
            ("create", "create file", "action://create"),
            ("move", "move file", "action://move"),
        ];

        for (cmd, desc, uri) in &commands {
            if cmd.starts_with(&lower) && *cmd != lower {
                // Score based on how much of the command is typed
                let typed_ratio = q16_div(
                    q16_from_int(partial.len() as i32),
                    q16_from_int(cmd.len() as i32),
                );
                let base_score = q16_mul(Q16_ONE, typed_ratio);

                // Boost if this command was recently used
                let recency_boost = if self
                    .recent_actions
                    .iter()
                    .rev()
                    .take(10)
                    .any(|a| a.starts_with(*cmd))
                {
                    Q16_ONE / 3
                } else {
                    Q16_ZERO
                };

                suggestions.push(Suggestion {
                    suggestion_type: SuggestionType::Text,
                    title: String::from(*cmd),
                    subtitle: String::from(*desc),
                    confidence: base_score + recency_boost,
                    action_uri: String::from(*uri),
                    icon: String::new(),
                });
            }
        }

        suggestions.sort_by(|a, b| b.confidence.cmp(&a.confidence));
        suggestions.truncate(max);
        suggestions
    }

    /// Get total number of tracked patterns
    pub fn pattern_count(&self) -> usize {
        self.patterns.len()
    }

    /// Get total number of sequence patterns
    pub fn sequence_count(&self) -> usize {
        self.sequences.len()
    }

    /// Prune stale patterns older than the given threshold (seconds)
    pub fn prune_stale(&mut self, max_age_secs: u64) {
        let now = crate::time::clock::unix_time();
        self.patterns
            .retain(|p| now.saturating_sub(p.last_used) < max_age_secs);
    }

    /// Adjust scoring weights (all in Q16)
    pub fn set_weights(&mut self, time: i32, freq: i32, seq: i32, recency: i32) {
        self.time_weight = time;
        self.freq_weight = freq;
        self.seq_weight = seq;
        self.recency_weight = recency;
    }

    /// Set context signal
    pub fn set_battery(&mut self, level: u8, charging: bool) {
        self.context.battery_level = level;
        self.context.is_charging = charging;
    }

    pub fn set_headphones(&mut self, connected: bool) {
        self.context.headphones_connected = connected;
    }

    pub fn set_notifications(&mut self, count: u32) {
        self.context.notifications_pending = count;
    }
}

/// Integer square root (not Q16)
fn q16_isqrt(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ---------------------------------------------------------------------------
// Global state and public API
// ---------------------------------------------------------------------------

static ENGINE: Mutex<SuggestionEngine> = Mutex::new(SuggestionEngine::new());

pub fn init() {
    crate::serial_println!("    [suggest] Smart suggestions initialized (Q16 scoring, temporal decay, sequence patterns)");
}

pub fn record_app(app_id: &str) {
    ENGINE.lock().record_app_usage(app_id);
}

pub fn record_action(action: &str) {
    ENGINE.lock().record_action(action);
}

pub fn suggest_apps(max: usize) -> Vec<Suggestion> {
    ENGINE.lock().suggest_apps(max)
}

pub fn suggest_replies(msg: &str) -> Vec<String> {
    ENGINE.lock().suggest_replies(msg)
}

pub fn suggest_actions() -> Vec<Suggestion> {
    ENGINE.lock().suggest_actions()
}

pub fn suggest_text(partial: &str, max: usize) -> Vec<Suggestion> {
    ENGINE.lock().suggest_text(partial, max)
}

pub fn update_context(signal: ContextSignal, value: u32) {
    ENGINE.lock().update_context(signal, value);
}

pub fn prune_stale(max_age_secs: u64) {
    ENGINE.lock().prune_stale(max_age_secs);
}

pub fn pattern_count() -> usize {
    ENGINE.lock().pattern_count()
}

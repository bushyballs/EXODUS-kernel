use crate::sync::Mutex;
/// User behavior profile for Genesis learning subsystem
///
/// Maintains a persistent model of user behavior characteristics:
///   - Learning rate: how quickly the system adapts to new patterns
///   - Confidence decay: how fast old patterns lose relevance
///   - Habit scoring: quantify how habitual each behavior is
///   - Expertise estimation: beginner vs power-user heuristics
///   - Behavioral clusters: group similar usage patterns
///   - Temporal profile: morning/afternoon/evening usage character
///
/// All math is Q16 fixed-point (i32, 16 fractional bits).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 fixed-point ────────────────────────────────────────────────────────

const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;
const Q16_QUARTER: i32 = 16384;
const Q16_TENTH: i32 = 6554;
const Q16_HUNDREDTH: i32 = 655;
const Q16_TWO: i32 = 131072;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Integer square root via Newton's method
fn isqrt(v: i32) -> i32 {
    if v <= 0 {
        return 0;
    }
    let val = v as u32;
    let mut guess = val;
    loop {
        let next = (guess + val / guess) / 2;
        if next >= guess {
            break;
        }
        guess = next;
    }
    guess as i32
}

// ── Configuration ──────────────────────────────────────────────────────────

const MAX_HABITS: usize = 64;
const MAX_BEHAVIOR_DIMENSIONS: usize = 12;
const TIME_PERIODS: usize = 4; // morning, afternoon, evening, night
const EXPERTISE_LEVELS: usize = 5; // novice, beginner, intermediate, advanced, expert

// ── Types ──────────────────────────────────────────────────────────────────

/// Expertise level based on observed behavior complexity
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpertiseLevel {
    Novice,
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}

/// Time period for temporal profiling
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimePeriod {
    Morning,   // 06:00-11:59
    Afternoon, // 12:00-17:59
    Evening,   // 18:00-22:59
    Night,     // 23:00-05:59
}

impl TimePeriod {
    pub fn from_hour(hour: u8) -> Self {
        match hour {
            6..=11 => TimePeriod::Morning,
            12..=17 => TimePeriod::Afternoon,
            18..=22 => TimePeriod::Evening,
            _ => TimePeriod::Night,
        }
    }

    pub fn index(self) -> usize {
        match self {
            TimePeriod::Morning => 0,
            TimePeriod::Afternoon => 1,
            TimePeriod::Evening => 2,
            TimePeriod::Night => 3,
        }
    }
}

/// A single tracked habit
pub struct Habit {
    pub id: u16,
    pub app_id: u16,
    pub description_code: u16, // localized description ID
    pub regularity: i32,       // Q16 [0..1] how consistent the pattern is
    pub strength: i32,         // Q16 [0..1] how ingrained the habit is
    pub streak_days: u32,      // consecutive days this habit occurred
    pub longest_streak: u32,
    pub total_occurrences: u32,
    pub expected_hour: u8,     // when this habit typically fires
    pub expected_day_mask: u8, // which days (bit flags)
    pub last_occurrence: u64,  // timestamp
    pub variance: i32,         // Q16 time variance (lower = more regular)
    pub active: bool,
}

impl Habit {
    fn new(id: u16, app_id: u16, hour: u8) -> Self {
        Habit {
            id,
            app_id,
            description_code: 0,
            regularity: Q16_ZERO,
            strength: Q16_TENTH,
            streak_days: 0,
            longest_streak: 0,
            total_occurrences: 0,
            expected_hour: hour,
            expected_day_mask: 0x7F, // all days by default
            last_occurrence: 0,
            variance: Q16_ONE, // high variance initially
            active: true,
        }
    }

    /// Update habit after an occurrence
    fn record_occurrence(&mut self, timestamp: u64, hour: u8) {
        self.total_occurrences = self.total_occurrences.saturating_add(1);

        // Update time variance (how close to expected hour)
        let hour_diff = if hour > self.expected_hour {
            (hour - self.expected_hour) as i32
        } else {
            (self.expected_hour - hour) as i32
        };
        let hour_diff = if hour_diff > 12 {
            24 - hour_diff
        } else {
            hour_diff
        };
        let time_error = (hour_diff as i32) * Q16_ONE;

        // EMA of variance: 0.8 * old + 0.2 * new
        self.variance = q16_mul(self.variance, 52429)  // 0.8
            + q16_mul(time_error, 13107); // 0.2

        // Regularity: inverse of variance (lower variance = more regular)
        if self.variance > Q16_HUNDREDTH {
            self.regularity =
                q16_clamp(q16_div(Q16_ONE, self.variance + Q16_ONE), Q16_ZERO, Q16_ONE);
        }

        // Strength: grows with occurrences using sigmoid-like curve
        // strength = occurrences / (occurrences + 20)
        let occ = self.total_occurrences as i32;
        self.strength = q16_clamp(q16_div(occ, occ + 20), Q16_ZERO, Q16_ONE);

        // Streak tracking
        let gap_ticks = timestamp.saturating_sub(self.last_occurrence);
        let approx_hours = gap_ticks / 3600000; // rough: 1 tick = 1 ms
        if approx_hours < 36 {
            // Within ~1.5 days: continue streak
            self.streak_days = self.streak_days.saturating_add(1);
            if self.streak_days > self.longest_streak {
                self.longest_streak = self.streak_days;
            }
        } else {
            // Streak broken
            self.streak_days = 1;
        }

        self.last_occurrence = timestamp;
    }

    /// Apply daily decay when a habit doesn't fire
    fn decay(&mut self) {
        if self.streak_days > 0 {
            self.streak_days = 0; // streak broken
        }
        // Slow strength decay
        self.strength = q16_mul(self.strength, 64880); // 0.99 per day
        self.regularity = q16_mul(self.regularity, 62259); // 0.95

        if self.strength < Q16_HUNDREDTH {
            self.active = false;
        }
    }

    /// Composite habit score: weighted combination of regularity, strength, streak
    pub fn score(&self) -> i32 {
        if !self.active {
            return Q16_ZERO;
        }

        let streak_bonus = q16_clamp(
            q16_div(self.streak_days as i32, self.streak_days as i32 + 7),
            Q16_ZERO,
            Q16_HALF,
        );

        // 40% regularity + 40% strength + 20% streak
        let reg_part = q16_mul(self.regularity, 26214); // 0.4
        let str_part = q16_mul(self.strength, 26214); // 0.4
        let strk_part = q16_mul(streak_bonus, 13107); // 0.2

        q16_clamp(reg_part + str_part + strk_part, Q16_ZERO, Q16_ONE)
    }
}

/// Behavioral dimension for clustering/profiling
pub struct BehaviorDimension {
    pub name_code: u16,  // dimension name ID
    pub value: i32,      // Q16 [0..1] current value
    pub confidence: i32, // Q16 how confident we are
    pub samples: u32,
}

/// Temporal usage profile (per time period)
pub struct TemporalProfile {
    pub activity_level: i32,    // Q16 [0..1] overall activity
    pub app_diversity: i32,     // Q16 [0..1] how many different apps
    pub interaction_depth: i32, // Q16 [0..1] session length
    pub total_sessions: u32,
}

/// The main user profile
pub struct UserProfile {
    pub enabled: bool,

    // Adaptive parameters
    pub learning_rate: i32,      // Q16 [0..1] how fast to adapt
    pub confidence_decay: i32,   // Q16 [0..1] daily decay multiplier
    pub novelty_preference: i32, // Q16 [0..1] preference for new vs familiar

    // Expertise
    pub expertise: ExpertiseLevel,
    pub expertise_score: i32,      // Q16 continuous score
    pub unique_features_used: u32, // number of distinct features touched
    pub shortcut_usage_rate: i32,  // Q16 keyboard shortcuts vs menus
    pub error_recovery_speed: i32, // Q16 how fast they undo mistakes

    // Habits
    pub habits: Vec<Habit>,
    pub next_habit_id: u16,

    // Behavioral dimensions (for clustering)
    pub dimensions: Vec<BehaviorDimension>,

    // Temporal profiles
    pub temporal: [TemporalProfile; TIME_PERIODS],

    // Meta-stats
    pub total_interactions: u64,
    pub days_active: u32,
    pub profile_created: u64,
    pub last_updated: u64,
}

impl UserProfile {
    const fn new() -> Self {
        UserProfile {
            enabled: true,
            learning_rate: Q16_HALF,
            confidence_decay: 62259, // 0.95 per day
            novelty_preference: Q16_HALF,
            expertise: ExpertiseLevel::Beginner,
            expertise_score: Q16_QUARTER,
            unique_features_used: 0,
            shortcut_usage_rate: Q16_ZERO,
            error_recovery_speed: Q16_HALF,
            habits: Vec::new(),
            next_habit_id: 1,
            dimensions: Vec::new(),
            temporal: [
                TemporalProfile {
                    activity_level: Q16_ZERO,
                    app_diversity: Q16_ZERO,
                    interaction_depth: Q16_ZERO,
                    total_sessions: 0,
                },
                TemporalProfile {
                    activity_level: Q16_ZERO,
                    app_diversity: Q16_ZERO,
                    interaction_depth: Q16_ZERO,
                    total_sessions: 0,
                },
                TemporalProfile {
                    activity_level: Q16_ZERO,
                    app_diversity: Q16_ZERO,
                    interaction_depth: Q16_ZERO,
                    total_sessions: 0,
                },
                TemporalProfile {
                    activity_level: Q16_ZERO,
                    app_diversity: Q16_ZERO,
                    interaction_depth: Q16_ZERO,
                    total_sessions: 0,
                },
            ],
            total_interactions: 0,
            days_active: 0,
            profile_created: 0,
            last_updated: 0,
        }
    }

    /// Record an interaction and update profile
    pub fn record_interaction(
        &mut self,
        app_id: u16,
        hour: u8,
        timestamp: u64,
        used_shortcut: bool,
    ) {
        if !self.enabled {
            return;
        }

        self.total_interactions = self.total_interactions.saturating_add(1);
        self.last_updated = timestamp;

        // Update temporal profile
        let period = TimePeriod::from_hour(hour);
        let tp = &mut self.temporal[period.index()];
        tp.total_sessions = tp.total_sessions.saturating_add(1);
        // Activity level: EMA
        tp.activity_level = q16_mul(tp.activity_level, 58982) // 0.9
            + Q16_TENTH;
        tp.activity_level = q16_clamp(tp.activity_level, Q16_ZERO, Q16_ONE);

        // Shortcut usage tracking
        if used_shortcut {
            self.shortcut_usage_rate = q16_mul(self.shortcut_usage_rate, 58982) // 0.9
                + Q16_TENTH;
        } else {
            self.shortcut_usage_rate = q16_mul(self.shortcut_usage_rate, 58982);
            // 0.9 decay only
        }
        self.shortcut_usage_rate = q16_clamp(self.shortcut_usage_rate, Q16_ZERO, Q16_ONE);

        // Update habit tracking
        self.update_habits(app_id, hour, timestamp);

        // Update expertise
        self.update_expertise();
    }

    /// Check if a recurring pattern constitutes a habit
    fn update_habits(&mut self, app_id: u16, hour: u8, timestamp: u64) {
        // Find existing habit for this app at this time
        let mut found = false;
        for habit in self.habits.iter_mut() {
            if habit.app_id == app_id && habit.active {
                let hour_diff = if hour > habit.expected_hour {
                    hour - habit.expected_hour
                } else {
                    habit.expected_hour - hour
                };
                // Within 2-hour window of expected time -> same habit
                if hour_diff <= 2 || hour_diff >= 22 {
                    habit.record_occurrence(timestamp, hour);
                    found = true;
                    break;
                }
            }
        }

        if !found && self.habits.len() < MAX_HABITS {
            let id = self.next_habit_id;
            self.next_habit_id = self.next_habit_id.saturating_add(1);
            let mut habit = Habit::new(id, app_id, hour);
            habit.record_occurrence(timestamp, hour);
            self.habits.push(habit);
        }
    }

    /// Recalculate expertise level from usage signals
    fn update_expertise(&mut self) {
        // Expertise factors:
        //   1. Shortcut usage (power users use shortcuts)
        //   2. Feature breadth (power users explore more features)
        //   3. Error recovery speed (power users recover faster)
        //   4. Interaction volume (more usage = more learning)

        let shortcut_factor = self.shortcut_usage_rate;

        let feature_factor = q16_clamp(
            q16_div(
                self.unique_features_used as i32,
                self.unique_features_used as i32 + 50,
            ),
            Q16_ZERO,
            Q16_ONE,
        );

        let recovery_factor = self.error_recovery_speed;

        let volume_factor = q16_clamp(
            q16_div(
                (self.total_interactions / 100) as i32,
                (self.total_interactions / 100) as i32 + 20,
            ),
            Q16_ZERO,
            Q16_ONE,
        );

        // Weighted combination: 30% shortcuts, 25% features, 20% recovery, 25% volume
        let score = q16_mul(shortcut_factor, 19661)   // 0.30
            + q16_mul(feature_factor, 16384)            // 0.25
            + q16_mul(recovery_factor, 13107)           // 0.20
            + q16_mul(volume_factor, 16384); // 0.25

        // Smooth toward new score (don't jump levels suddenly)
        self.expertise_score = q16_mul(self.expertise_score, 52429)  // 0.8
            + q16_mul(score, 13107); // 0.2

        // Map score to level
        self.expertise = if self.expertise_score < 13107 {
            ExpertiseLevel::Novice
        } else if self.expertise_score < 26214 {
            ExpertiseLevel::Beginner
        } else if self.expertise_score < 39322 {
            ExpertiseLevel::Intermediate
        } else if self.expertise_score < 52429 {
            ExpertiseLevel::Advanced
        } else {
            ExpertiseLevel::Expert
        };
    }

    /// Record that the user used a previously unused feature
    pub fn record_feature_use(&mut self, _feature_id: u32) {
        self.unique_features_used = self.unique_features_used.saturating_add(1);
    }

    /// Record error recovery speed (0 = slow, Q16_ONE = instant)
    pub fn record_error_recovery(&mut self, speed: i32) {
        let clamped = q16_clamp(speed, Q16_ZERO, Q16_ONE);
        self.error_recovery_speed = q16_mul(self.error_recovery_speed, 52429) // 0.8
            + q16_mul(clamped, 13107); // 0.2
    }

    /// Apply daily decay to all habits and profile data
    pub fn daily_decay(&mut self) {
        self.days_active = self.days_active.saturating_add(1);

        for habit in self.habits.iter_mut() {
            // Check if habit fired today (simplified: check last_occurrence)
            // In production, this would check against today's date
            habit.decay();
        }

        // Prune dead habits
        self.habits.retain(|h| h.active);

        // Decay temporal profiles
        for tp in self.temporal.iter_mut() {
            tp.activity_level = q16_mul(tp.activity_level, 64880); // 0.99
        }

        // Adjust learning rate based on expertise
        // Experts need less aggressive learning; novices need more
        self.learning_rate = match self.expertise {
            ExpertiseLevel::Novice => 45875,       // 0.70
            ExpertiseLevel::Beginner => 39322,     // 0.60
            ExpertiseLevel::Intermediate => 32768, // 0.50
            ExpertiseLevel::Advanced => 26214,     // 0.40
            ExpertiseLevel::Expert => 19661,       // 0.30
        };
    }

    /// Get the strongest habits (sorted by score descending)
    pub fn top_habits(&self, count: usize) -> Vec<(u16, i32)> {
        let mut scored: Vec<(u16, i32)> = self
            .habits
            .iter()
            .filter(|h| h.active)
            .map(|h| (h.id, h.score()))
            .collect();

        // Insertion sort descending
        for i in 1..scored.len() {
            let mut j = i;
            while j > 0 && scored[j].1 > scored[j - 1].1 {
                scored.swap(j, j - 1);
                j -= 1;
            }
        }

        scored.truncate(count);
        scored
    }

    /// Get the most active time period
    pub fn peak_activity_period(&self) -> TimePeriod {
        let mut best_idx = 0;
        let mut best_level = Q16_ZERO;
        for i in 0..TIME_PERIODS {
            if self.temporal[i].activity_level > best_level {
                best_level = self.temporal[i].activity_level;
                best_idx = i;
            }
        }
        match best_idx {
            0 => TimePeriod::Morning,
            1 => TimePeriod::Afternoon,
            2 => TimePeriod::Evening,
            _ => TimePeriod::Night,
        }
    }

    /// Compute a behavioral fingerprint: a vector of dimension values
    pub fn behavioral_fingerprint(&self) -> Vec<i32> {
        let mut fp = Vec::new();
        fp.push(self.expertise_score);
        fp.push(self.shortcut_usage_rate);
        fp.push(self.error_recovery_speed);
        fp.push(self.novelty_preference);
        for tp in &self.temporal {
            fp.push(tp.activity_level);
        }
        fp
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static PROFILE: Mutex<Option<UserProfile>> = Mutex::new(None);

pub fn init() {
    let mut guard = PROFILE.lock();
    *guard = Some(UserProfile::new());
    serial_println!("    [learning] User profile initialized");
}

/// Record a user interaction
pub fn record_interaction(app_id: u16, hour: u8, timestamp: u64, used_shortcut: bool) {
    let mut guard = PROFILE.lock();
    if let Some(profile) = guard.as_mut() {
        profile.record_interaction(app_id, hour, timestamp, used_shortcut);
    }
}

/// Record use of a new feature
pub fn record_feature(feature_id: u32) {
    let mut guard = PROFILE.lock();
    if let Some(profile) = guard.as_mut() {
        profile.record_feature_use(feature_id);
    }
}

/// Get the current expertise level
pub fn expertise() -> ExpertiseLevel {
    let guard = PROFILE.lock();
    if let Some(profile) = guard.as_ref() {
        profile.expertise
    } else {
        ExpertiseLevel::Beginner
    }
}

/// Get current learning rate (Q16)
pub fn learning_rate() -> i32 {
    let guard = PROFILE.lock();
    if let Some(profile) = guard.as_ref() {
        profile.learning_rate
    } else {
        Q16_HALF
    }
}

/// Run daily maintenance
pub fn daily_maintenance() {
    let mut guard = PROFILE.lock();
    if let Some(profile) = guard.as_mut() {
        profile.daily_decay();
    }
}

/// Get top habits
pub fn top_habits(count: usize) -> Vec<(u16, i32)> {
    let guard = PROFILE.lock();
    if let Some(profile) = guard.as_ref() {
        profile.top_habits(count)
    } else {
        Vec::new()
    }
}

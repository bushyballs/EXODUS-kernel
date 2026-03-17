use crate::sync::Mutex;
/// User preference tracking — the AI's memory of YOU
///
/// Tracks and remembers everything about user preferences,
/// wants, habits, dislikes, and interaction patterns. The AI
/// uses this to personalize every single interaction.
///
/// This is YOUR data, stored locally, never leaving the device.
/// The AI learns how YOU like to work, what you care about,
/// and what you never want to see again.
///
/// Features:
///   - Explicit preferences (user stated directly)
///   - Inferred preferences (AI figured out from behavior)
///   - Habit detection (recurring patterns over time)
///   - Conversation style adaptation (formality, humor, detail)
///   - Dislike tracking (topics to avoid)
///   - Preference decay (old unused prefs weaken over time)
///   - Profile completeness scoring
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

// ── Constants ──────────────────────────────────────────────

/// Q16 representation of 0.5 (32768 = 0.5 * 65536)
const Q16_HALF: Q16 = 32768;

/// Q16 representation of 1.0
const Q16_ONE: Q16 = 65536;

/// Q16 representation of 0.25
const Q16_QUARTER: Q16 = 16384;

/// Q16 representation of 0.75
const Q16_THREE_QUARTER: Q16 = 49152;

/// Q16 representation of 0.1
const Q16_TENTH: Q16 = 6554;

/// Seconds in one day (86400)
const SECONDS_PER_DAY: u64 = 86400;

/// After this many seconds without use, wants start decaying (30 days)
const WANT_DECAY_THRESHOLD: u64 = 30 * SECONDS_PER_DAY;

/// Maximum number of habit observations before we auto-detect
const HABIT_DETECTION_THRESHOLD: u32 = 5;

/// Maximum habits to track
const MAX_HABITS: usize = 128;

/// Maximum wants to track
const MAX_WANTS: usize = 512;

/// Maximum dislikes to track
const MAX_DISLIKES: usize = 256;

// ── Enums ──────────────────────────────────────────────────

/// How the preference was established
#[derive(Clone, Copy, PartialEq)]
pub enum PreferenceSource {
    /// User explicitly stated this preference
    Explicit,
    /// AI inferred from behavior patterns
    Inferred,
    /// User corrected an AI assumption
    Corrected,
    /// System default, not yet personalized
    Default,
}

/// How strongly the user feels about this preference
#[derive(Clone, Copy, PartialEq)]
pub enum PreferenceStrength {
    /// Slight preference, easily overridden
    Weak,
    /// Noticeable preference, consider it
    Moderate,
    /// Clear preference, respect it
    Strong,
    /// Non-negotiable, always honor this
    Absolute,
}

// ── Data Structures ────────────────────────────────────────

/// Something the user wants, needs, or cares about
#[derive(Clone)]
pub struct UserWant {
    pub id: u32,
    pub description_hash: u64,
    pub category_hash: u64,
    pub strength: PreferenceStrength,
    pub source: PreferenceSource,
    pub created: u64,
    pub last_used: u64,
    pub use_count: u32,
    pub still_valid: bool,
}

/// A detected behavioral pattern — something the user does regularly
#[derive(Clone, Copy)]
pub struct UserHabit {
    pub pattern_hash: u64,
    /// Times per day in Q16 fixed-point
    pub frequency: Q16,
    /// Minutes from midnight (0-1439)
    pub time_of_day: u32,
    /// Bitmask of days: bit 0 = Monday, bit 6 = Sunday
    pub day_pattern: u8,
    /// Confidence in this habit (Q16, 0 to Q16_ONE)
    pub confidence: Q16,
    /// How many times this pattern has been observed
    pub detected_count: u32,
}

/// Something the user dislikes or wants to avoid
#[derive(Clone, Copy)]
pub struct UserDislike {
    pub topic_hash: u64,
    pub strength: PreferenceStrength,
    pub source: PreferenceSource,
    pub timestamp: u64,
}

/// How the user prefers to interact — adapted per conversation
#[derive(Clone, Copy)]
pub struct ConversationStyle {
    /// 0 = very casual, Q16_ONE = very formal
    pub formality: Q16,
    /// 0 = no humor, Q16_ONE = very humorous
    pub humor: Q16,
    /// 0 = terse/minimal, Q16_ONE = extremely detailed
    pub detail_level: Q16,
    /// 0 = impatient (short answers), Q16_ONE = very patient (long explanations)
    pub patience: Q16,
    /// 0 = complete beginner, Q16_ONE = domain expert
    pub tech_level: Q16,
}

/// Central store for all user preferences
pub struct PreferenceStore {
    pub wants: Vec<UserWant>,
    pub habits: Vec<UserHabit>,
    pub dislikes: Vec<UserDislike>,
    pub style: ConversationStyle,
    pub next_want_id: u32,
    pub total_observations: u64,
    pub profile_completeness: Q16,
}

// ── Raw observation buffer for habit detection ─────────────

#[derive(Clone, Copy)]
struct HabitObservation {
    pattern_hash: u64,
    time_of_day: u32,
    day_of_week: u8,
    timestamp: u64,
}

static STORE: Mutex<Option<PreferenceStore>> = Mutex::new(None);
static OBSERVATIONS: Mutex<Option<Vec<HabitObservation>>> = Mutex::new(None);

// ── Implementation ─────────────────────────────────────────

impl ConversationStyle {
    /// Default moderate style — middle ground on everything
    pub fn default_style() -> Self {
        ConversationStyle {
            formality: Q16_HALF,
            humor: Q16_HALF,
            detail_level: Q16_HALF,
            patience: Q16_HALF,
            tech_level: Q16_HALF,
        }
    }
}

impl PreferenceStrength {
    /// Convert strength to a Q16 weight for scoring
    pub fn to_q16(&self) -> Q16 {
        match self {
            PreferenceStrength::Weak => Q16_QUARTER,
            PreferenceStrength::Moderate => Q16_HALF,
            PreferenceStrength::Strong => Q16_THREE_QUARTER,
            PreferenceStrength::Absolute => Q16_ONE,
        }
    }
}

impl PreferenceStore {
    /// Create a new empty preference store with default style
    pub fn new() -> Self {
        PreferenceStore {
            wants: Vec::new(),
            habits: Vec::new(),
            dislikes: Vec::new(),
            style: ConversationStyle::default_style(),
            next_want_id: 1,
            total_observations: 0,
            profile_completeness: 0,
        }
    }

    /// Add a new want/preference. Returns the assigned ID.
    pub fn add_want(
        &mut self,
        desc: u64,
        cat: u64,
        strength: PreferenceStrength,
        source: PreferenceSource,
        time: u64,
    ) -> u32 {
        // Check if we already have this want (by description hash)
        let mut found_id = None;
        for want in self.wants.iter_mut() {
            if want.description_hash == desc {
                // Update existing want
                want.strength = strength;
                want.source = source;
                want.last_used = time;
                want.use_count += 1;
                want.still_valid = true;
                found_id = Some(want.id);
                break;
            }
        }
        if let Some(id) = found_id {
            self.total_observations = self.total_observations.saturating_add(1);
            self.recalculate_completeness();
            return id;
        }

        // Enforce capacity limit — remove weakest if full
        if self.wants.len() >= MAX_WANTS {
            self.evict_weakest_want();
        }

        let id = self.next_want_id;
        self.next_want_id = self.next_want_id.saturating_add(1);

        self.wants.push(UserWant {
            id,
            description_hash: desc,
            category_hash: cat,
            strength,
            source,
            created: time,
            last_used: time,
            use_count: 1,
            still_valid: true,
        });

        self.total_observations = self.total_observations.saturating_add(1);
        self.recalculate_completeness();
        id
    }

    /// Remove a want by ID
    pub fn remove_want(&mut self, id: u32) {
        self.wants.retain(|w| w.id != id);
        self.recalculate_completeness();
    }

    /// Record a habit observation for later pattern detection
    pub fn record_habit(&mut self, pattern: u64, time_of_day: u32, day: u8) {
        // Check if we already track this habit
        let mut found = false;
        for habit in self.habits.iter_mut() {
            if habit.pattern_hash == pattern {
                habit.detected_count += 1;
                // Update day pattern bitmask
                habit.day_pattern |= 1 << (day & 0x07);
                // Running average of time of day using Q16 math
                let old_time = q16_from_int(habit.time_of_day as i32);
                let new_time = q16_from_int(time_of_day as i32);
                // Weighted average: 0.75 * old + 0.25 * new
                let blended = q16_mul(old_time, Q16_THREE_QUARTER) + q16_mul(new_time, Q16_QUARTER);
                habit.time_of_day = (blended >> 16) as u32;
                // Increase confidence as we see more data
                if habit.confidence < Q16_ONE {
                    habit.confidence += Q16_TENTH;
                    if habit.confidence > Q16_ONE {
                        habit.confidence = Q16_ONE;
                    }
                }
                found = true;
                break;
            }
        }

        if !found {
            // Store as observation for later detection
            let mut obs_guard = OBSERVATIONS.lock();
            if let Some(ref mut obs) = *obs_guard {
                obs.push(HabitObservation {
                    pattern_hash: pattern,
                    time_of_day,
                    day_of_week: day,
                    timestamp: 0,
                });
            }
        }

        self.total_observations = self.total_observations.saturating_add(1);
    }

    /// Analyze recorded observations to identify recurring habits
    pub fn detect_habits(&mut self) {
        let mut obs_guard = OBSERVATIONS.lock();
        let observations = match obs_guard.as_mut() {
            Some(o) => o,
            None => return,
        };

        if observations.is_empty() {
            return;
        }

        // Group observations by pattern_hash and count occurrences
        let mut pattern_counts: Vec<(u64, u32, u32, u8)> = Vec::new(); // (hash, count, avg_time, day_bits)

        for obs in observations.iter() {
            let mut found = false;
            for entry in pattern_counts.iter_mut() {
                if entry.0 == obs.pattern_hash {
                    entry.1 += 1;
                    // Running average of time
                    let total_time = (entry.2 as u64 * (entry.1 as u64 - 1)
                        + obs.time_of_day as u64)
                        / entry.1 as u64;
                    entry.2 = total_time as u32;
                    entry.3 |= 1 << (obs.day_of_week & 0x07);
                    found = true;
                    break;
                }
            }
            if !found {
                pattern_counts.push((
                    obs.pattern_hash,
                    1,
                    obs.time_of_day,
                    1 << (obs.day_of_week & 0x07),
                ));
            }
        }

        // Promote observations with enough data points to actual habits
        for (hash, count, avg_time, day_bits) in pattern_counts.iter() {
            if *count >= HABIT_DETECTION_THRESHOLD {
                // Check if this habit already exists
                let already_tracked = self.habits.iter().any(|h| h.pattern_hash == *hash);
                if !already_tracked && self.habits.len() < MAX_HABITS {
                    // Calculate frequency: count per active days
                    let active_days = count_bits(*day_bits) as i32;
                    let freq = if active_days > 0 {
                        q16_from_int(*count as i32) / active_days
                    } else {
                        q16_from_int(1)
                    };

                    // Initial confidence based on observation count
                    let confidence = if *count >= 20 {
                        Q16_THREE_QUARTER
                    } else if *count >= 10 {
                        Q16_HALF
                    } else {
                        Q16_QUARTER
                    };

                    self.habits.push(UserHabit {
                        pattern_hash: *hash,
                        frequency: freq,
                        time_of_day: *avg_time,
                        day_pattern: *day_bits,
                        confidence,
                        detected_count: *count,
                    });
                }
            }
        }

        // Clear processed observations that became habits
        observations.retain(|obs| {
            !self
                .habits
                .iter()
                .any(|h| h.pattern_hash == obs.pattern_hash)
        });

        self.recalculate_completeness();
    }

    /// Add a topic the user dislikes
    pub fn add_dislike(
        &mut self,
        topic: u64,
        strength: PreferenceStrength,
        source: PreferenceSource,
        time: u64,
    ) {
        // Update existing dislike if present
        for dislike in self.dislikes.iter_mut() {
            if dislike.topic_hash == topic {
                dislike.strength = strength;
                dislike.source = source;
                dislike.timestamp = time;
                self.total_observations = self.total_observations.saturating_add(1);
                return;
            }
        }

        // Enforce capacity limit
        if self.dislikes.len() >= MAX_DISLIKES {
            // Remove oldest dislike
            if !self.dislikes.is_empty() {
                let mut oldest_idx = 0;
                let mut oldest_time = u64::MAX;
                for (i, d) in self.dislikes.iter().enumerate() {
                    if d.timestamp < oldest_time {
                        oldest_time = d.timestamp;
                        oldest_idx = i;
                    }
                }
                self.dislikes.remove(oldest_idx);
            }
        }

        self.dislikes.push(UserDislike {
            topic_hash: topic,
            strength,
            source,
            timestamp: time,
        });

        self.total_observations = self.total_observations.saturating_add(1);
        self.recalculate_completeness();
    }

    /// Update conversation style — only modifies fields that are Some
    pub fn update_style(
        &mut self,
        formality: Option<Q16>,
        humor: Option<Q16>,
        detail: Option<Q16>,
        patience: Option<Q16>,
        tech: Option<Q16>,
    ) {
        if let Some(f) = formality {
            self.style.formality = clamp_q16(f);
        }
        if let Some(h) = humor {
            self.style.humor = clamp_q16(h);
        }
        if let Some(d) = detail {
            self.style.detail_level = clamp_q16(d);
        }
        if let Some(p) = patience {
            self.style.patience = clamp_q16(p);
        }
        if let Some(t) = tech {
            self.style.tech_level = clamp_q16(t);
        }
        self.total_observations = self.total_observations.saturating_add(1);
    }

    /// Infer conversation style adjustments from a single interaction
    ///
    /// Gradually shifts style based on observed signals:
    /// - Long messages suggest the user wants detail
    /// - Jargon usage suggests higher tech level
    /// - Follow-up questions suggest patience/curiosity
    pub fn infer_style_from_interaction(
        &mut self,
        msg_length: u32,
        used_jargon: bool,
        asked_followup: bool,
    ) {
        // Nudge factor — small adjustments per interaction
        let nudge = Q16_TENTH; // 0.1 per observation

        // Long messages (> 200 chars) suggest user likes detail
        if msg_length > 200 {
            let target_detail = Q16_THREE_QUARTER;
            if self.style.detail_level < target_detail {
                self.style.detail_level += nudge;
            }
        } else if msg_length < 50 {
            // Short messages suggest user prefers brevity
            let target_detail = Q16_QUARTER;
            if self.style.detail_level > target_detail {
                self.style.detail_level -= nudge;
            }
        }

        // Jargon usage increases perceived tech level
        if used_jargon {
            if self.style.tech_level < Q16_ONE - nudge {
                self.style.tech_level += nudge;
            }
            // Jargon users tend to prefer less formality
            if self.style.formality > nudge {
                self.style.formality -= nudge / 2;
            }
        }

        // Follow-up questions indicate patience and engagement
        if asked_followup {
            if self.style.patience < Q16_ONE - nudge {
                self.style.patience += nudge;
            }
            // Curious users often appreciate humor
            if self.style.humor < Q16_ONE - nudge {
                self.style.humor += nudge / 2;
            }
        }

        // Clamp all values
        self.style.formality = clamp_q16(self.style.formality);
        self.style.humor = clamp_q16(self.style.humor);
        self.style.detail_level = clamp_q16(self.style.detail_level);
        self.style.patience = clamp_q16(self.style.patience);
        self.style.tech_level = clamp_q16(self.style.tech_level);

        self.total_observations = self.total_observations.saturating_add(1);
    }

    /// Get wants that match a given topic hash
    pub fn get_relevant_wants(&self, topic: u64) -> Vec<&UserWant> {
        let mut results = Vec::new();
        for want in self.wants.iter() {
            if want.still_valid && want.category_hash == topic {
                results.push(want);
            }
        }
        // Sort by strength (strongest first) using a simple insertion sort
        // to avoid pulling in sort infrastructure
        let len = results.len();
        for i in 1..len {
            let mut j = i;
            while j > 0
                && strength_rank(results[j].strength) > strength_rank(results[j - 1].strength)
            {
                results.swap(j, j - 1);
                j -= 1;
            }
        }
        results
    }

    /// Check if a topic is disliked
    pub fn is_disliked(&self, topic: u64) -> bool {
        self.dislikes.iter().any(|d| d.topic_hash == topic)
    }

    /// Get current conversation style
    pub fn get_style(&self) -> &ConversationStyle {
        &self.style
    }

    /// Decay old unused wants — weaken preferences that haven't been
    /// referenced in a long time. Called periodically.
    pub fn decay_old_wants(&mut self, current_time: u64) {
        for want in self.wants.iter_mut() {
            if !want.still_valid {
                continue;
            }

            let age = current_time.saturating_sub(want.last_used);
            if age > WANT_DECAY_THRESHOLD {
                // Downgrade strength based on how long it's been unused
                match want.strength {
                    PreferenceStrength::Absolute => {
                        // Absolute preferences decay slower
                        if age > WANT_DECAY_THRESHOLD * 3 {
                            want.strength = PreferenceStrength::Strong;
                        }
                    }
                    PreferenceStrength::Strong => {
                        want.strength = PreferenceStrength::Moderate;
                    }
                    PreferenceStrength::Moderate => {
                        want.strength = PreferenceStrength::Weak;
                    }
                    PreferenceStrength::Weak => {
                        // Weak wants that are very old get invalidated
                        if age > WANT_DECAY_THRESHOLD * 4 {
                            want.still_valid = false;
                        }
                    }
                }
            }
        }
    }

    /// Calculate and return profile completeness as Q16
    ///
    /// Scoring breakdown (all in Q16):
    ///   - Has at least 5 wants: 0.2
    ///   - Has at least 2 habits: 0.2
    ///   - Has at least 1 dislike: 0.1
    ///   - Style diverges from default: 0.3
    ///   - 20+ total observations: 0.2
    pub fn get_completeness(&self) -> Q16 {
        self.profile_completeness
    }

    /// Get summary stats: (want_count, habit_count, dislike_count, completeness)
    pub fn get_stats(&self) -> (u32, u32, u32, Q16) {
        let valid_wants = self.wants.iter().filter(|w| w.still_valid).count() as u32;
        (
            valid_wants,
            self.habits.len() as u32,
            self.dislikes.len() as u32,
            self.profile_completeness,
        )
    }

    // ── Private helpers ────────────────────────────────────

    /// Recalculate profile completeness score
    fn recalculate_completeness(&mut self) {
        let mut score: Q16 = 0;

        // Has at least 5 valid wants: +0.2
        let valid_wants = self.wants.iter().filter(|w| w.still_valid).count();
        if valid_wants >= 5 {
            score += q16_from_int(1) / 5; // 0.2
        } else if valid_wants > 0 {
            // Partial credit
            score += (q16_from_int(1) / 5) * (valid_wants as i32) / 5;
        }

        // Has at least 2 habits: +0.2
        if self.habits.len() >= 2 {
            score += q16_from_int(1) / 5;
        } else if !self.habits.is_empty() {
            score += q16_from_int(1) / 10;
        }

        // Has at least 1 dislike: +0.1
        if !self.dislikes.is_empty() {
            score += q16_from_int(1) / 10;
        }

        // Style diverges from default: +0.3
        let default_style = ConversationStyle::default_style();
        let style_delta = abs_q16(self.style.formality - default_style.formality)
            + abs_q16(self.style.humor - default_style.humor)
            + abs_q16(self.style.detail_level - default_style.detail_level)
            + abs_q16(self.style.patience - default_style.patience)
            + abs_q16(self.style.tech_level - default_style.tech_level);
        // If total divergence > 0.5 (32768), full credit
        if style_delta > Q16_HALF {
            score += (q16_from_int(1) * 3) / 10; // 0.3
        } else if style_delta > 0 {
            // Partial: scale proportionally
            score += q16_mul(
                (q16_from_int(1) * 3) / 10,
                q16_mul(style_delta, q16_from_int(2)),
            );
        }

        // 20+ observations: +0.2
        if self.total_observations >= 20 {
            score += q16_from_int(1) / 5;
        } else if self.total_observations > 0 {
            score += (q16_from_int(1) / 5) * (self.total_observations as i32) / 20;
        }

        self.profile_completeness = clamp_q16(score);
    }

    /// Evict the weakest, oldest want to make room
    fn evict_weakest_want(&mut self) {
        if self.wants.is_empty() {
            return;
        }

        let mut worst_idx = 0;
        let mut worst_score: i32 = i32::MAX;

        for (i, want) in self.wants.iter().enumerate() {
            // Score: strength weight + recency bonus + use count bonus
            let strength_score = want.strength.to_q16();
            let use_bonus = q16_from_int(want.use_count.min(100) as i32) / 100;
            let valid_bonus = if want.still_valid { Q16_HALF } else { 0 };
            let total = strength_score + use_bonus + valid_bonus;

            if total < worst_score {
                worst_score = total;
                worst_idx = i;
            }
        }

        self.wants.remove(worst_idx);
    }
}

// ── Free functions ─────────────────────────────────────────

/// Clamp a Q16 value to the range [0, Q16_ONE]
fn clamp_q16(value: Q16) -> Q16 {
    if value < 0 {
        0
    } else if value > Q16_ONE {
        Q16_ONE
    } else {
        value
    }
}

/// Absolute value for Q16
fn abs_q16(value: Q16) -> Q16 {
    if value < 0 {
        -value
    } else {
        value
    }
}

/// Count set bits in a byte (population count)
fn count_bits(mut byte: u8) -> u8 {
    let mut count = 0u8;
    while byte != 0 {
        count += byte & 1;
        byte >>= 1;
    }
    count
}

/// Rank preference strength as integer for comparison
fn strength_rank(s: PreferenceStrength) -> u8 {
    match s {
        PreferenceStrength::Weak => 1,
        PreferenceStrength::Moderate => 2,
        PreferenceStrength::Strong => 3,
        PreferenceStrength::Absolute => 4,
    }
}

// ── Module init ────────────────────────────────────────────

pub fn init() {
    let mut store = STORE.lock();
    *store = Some(PreferenceStore::new());

    let mut obs = OBSERVATIONS.lock();
    *obs = Some(Vec::new());

    serial_println!("    Preference tracking initialized (wants, habits, dislikes, style)");
}

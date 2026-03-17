use crate::sync::Mutex;
/// Usage pattern detection for Genesis learning subsystem
///
/// Tracks and analyzes user behavior to detect recurring patterns:
///   - App usage frequency and time-of-day correlation
///   - Action sequences (app A -> app B within N seconds)
///   - Periodic habits (daily, weekly, time-bucketed)
///   - Frequency analysis with exponential decay
///   - Markov chain transition probabilities between actions
///
/// All math is Q16 fixed-point (i32, 16 fractional bits).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 fixed-point constants ──────────────────────────────────────────────

const Q16_ONE: i32 = 65536; // 1.0
const Q16_HALF: i32 = 32768; // 0.5
const Q16_ZERO: i32 = 0; // 0.0
const Q16_DECAY_FAST: i32 = 62259; // 0.95
const Q16_DECAY_SLOW: i32 = 64880; // 0.99
const Q16_TENTH: i32 = 6554; // 0.1
const Q16_HUNDREDTH: i32 = 655; // 0.01

/// Q16 multiply: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

/// Q16 divide: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

// ── Configuration ──────────────────────────────────────────────────────────

const MAX_TRACKED_APPS: usize = 128;
const MAX_SEQUENCES: usize = 256;
const MAX_EVENTS_HISTORY: usize = 1024;
const TIME_BUCKETS: usize = 24; // one per hour
const DAYS_OF_WEEK: usize = 7;
const TRANSITION_MATRIX_SIZE: usize = 64;

// ── Types ──────────────────────────────────────────────────────────────────

/// Identifies what kind of event was observed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    AppLaunch,
    AppClose,
    FileOpen,
    SearchQuery,
    SettingChange,
    SystemAction,
    InputGesture,
    NetworkRequest,
}

/// A single observed event with timestamp
#[derive(Clone)]
pub struct ObservedEvent {
    pub kind: EventKind,
    pub app_id: u16,
    pub timestamp: u64,    // kernel ticks
    pub hour: u8,          // 0-23
    pub day_of_week: u8,   // 0-6 (Mon-Sun)
    pub context_hash: u32, // hash of surrounding context
}

/// Per-app usage statistics
pub struct AppUsageStats {
    pub app_id: u16,
    pub total_launches: u32,
    pub hourly_freq: [i32; TIME_BUCKETS], // Q16 frequency per hour
    pub daily_freq: [i32; DAYS_OF_WEEK],  // Q16 frequency per day
    pub last_used: u64,
    pub avg_session_len: i32, // Q16 seconds
    pub session_count: u32,
    pub momentum: i32, // Q16 trending up/down
}

impl AppUsageStats {
    fn new(app_id: u16) -> Self {
        AppUsageStats {
            app_id,
            total_launches: 0,
            hourly_freq: [Q16_ZERO; TIME_BUCKETS],
            daily_freq: [Q16_ZERO; DAYS_OF_WEEK],
            last_used: 0,
            avg_session_len: 0,
            session_count: 0,
            momentum: 0,
        }
    }
}

/// A detected sequence: event A followed by event B
pub struct ActionSequence {
    pub first_app: u16,
    pub second_app: u16,
    pub avg_gap_ticks: u64,
    pub occurrences: u32,
    pub confidence: i32, // Q16 [0..Q16_ONE]
    pub last_seen: u64,
}

/// Markov transition cell: probability of going from state i to state j
pub struct TransitionCell {
    pub from_app: u16,
    pub to_app: u16,
    pub probability: i32, // Q16 [0..Q16_ONE]
    pub count: u32,
}

/// The main pattern detection engine
pub struct PatternEngine {
    pub enabled: bool,
    pub app_stats: Vec<AppUsageStats>,
    pub sequences: Vec<ActionSequence>,
    pub transitions: Vec<TransitionCell>,
    pub event_history: Vec<ObservedEvent>,
    pub history_write_idx: usize,
    pub total_events: u64,
    pub global_decay_rate: i32,  // Q16
    pub sequence_gap_max: u64,   // max ticks between A and B
    pub min_sequence_count: u32, // min occurrences to report
}

impl PatternEngine {
    const fn new() -> Self {
        PatternEngine {
            enabled: true,
            app_stats: Vec::new(),
            sequences: Vec::new(),
            transitions: Vec::new(),
            event_history: Vec::new(),
            history_write_idx: 0,
            total_events: 0,
            global_decay_rate: Q16_DECAY_FAST,
            sequence_gap_max: 30000, // ~30 seconds at 1kHz tick
            min_sequence_count: 3,
        }
    }

    /// Record an observed event and update all statistics
    pub fn record_event(&mut self, event: ObservedEvent) {
        if !self.enabled {
            return;
        }

        // Update app stats
        self.update_app_stats(&event);

        // Detect sequences from recent history
        self.detect_sequence(&event);

        // Update Markov transitions
        self.update_transitions(&event);

        // Push into ring buffer
        if self.event_history.len() < MAX_EVENTS_HISTORY {
            self.event_history.push(event);
        } else {
            let idx = self.history_write_idx % MAX_EVENTS_HISTORY;
            self.event_history[idx] = event;
        }
        self.history_write_idx += 1;
        self.total_events = self.total_events.saturating_add(1);
    }

    /// Find or create app stats for the given app_id
    fn find_or_create_app(&mut self, app_id: u16) -> usize {
        for i in 0..self.app_stats.len() {
            if self.app_stats[i].app_id == app_id {
                return i;
            }
        }
        if self.app_stats.len() < MAX_TRACKED_APPS {
            self.app_stats.push(AppUsageStats::new(app_id));
            self.app_stats.len() - 1
        } else {
            // Evict least-used app
            let mut min_idx = 0;
            let mut min_launches = u32::MAX;
            for i in 0..self.app_stats.len() {
                if self.app_stats[i].total_launches < min_launches {
                    min_launches = self.app_stats[i].total_launches;
                    min_idx = i;
                }
            }
            self.app_stats[min_idx] = AppUsageStats::new(app_id);
            min_idx
        }
    }

    /// Update per-app frequency and timing statistics
    fn update_app_stats(&mut self, event: &ObservedEvent) {
        let idx = self.find_or_create_app(event.app_id);
        let stats = &mut self.app_stats[idx];

        stats.total_launches = stats.total_launches.saturating_add(1);
        stats.last_used = event.timestamp;

        // Increment hourly frequency bucket (Q16 += 0.1 per event, capped at 1.0)
        let hour = (event.hour as usize) % TIME_BUCKETS;
        stats.hourly_freq[hour] = core::cmp::min(stats.hourly_freq[hour] + Q16_TENTH, Q16_ONE);

        // Increment daily frequency bucket
        let day = (event.day_of_week as usize) % DAYS_OF_WEEK;
        stats.daily_freq[day] = core::cmp::min(stats.daily_freq[day] + Q16_TENTH, Q16_ONE);

        // Update momentum: exponential moving average of inter-event gap
        // momentum > 0.5 means usage is increasing
        if stats.session_count > 0 {
            let recency = if event.timestamp > stats.last_used {
                event.timestamp - stats.last_used
            } else {
                1
            };
            // Short gap -> high momentum, long gap -> low momentum
            let gap_factor = if recency < 1000 {
                Q16_ONE // very recent -> max momentum
            } else if recency < 100000 {
                q16_div(1000_i32 * Q16_ONE / 1000, (recency as i32).max(1))
            } else {
                Q16_HUNDREDTH
            };
            // EMA: momentum = 0.9 * old + 0.1 * new
            let ema_old = q16_mul(stats.momentum, Q16_DECAY_FAST);
            let ema_new = q16_mul(gap_factor, Q16_TENTH);
            stats.momentum = ema_old + ema_new;
        }

        stats.session_count = stats.session_count.saturating_add(1);
    }

    /// Check if the new event forms a sequence with recent history
    fn detect_sequence(&mut self, event: &ObservedEvent) {
        if event.kind != EventKind::AppLaunch {
            return;
        }

        // Look backwards in history for a preceding AppLaunch
        let history_len = self.event_history.len();
        if history_len == 0 {
            return;
        }

        // Search last 8 events for a recent app launch
        let search_start = if history_len > 8 { history_len - 8 } else { 0 };
        for i in (search_start..history_len).rev() {
            let prev = &self.event_history[i];
            if prev.kind != EventKind::AppLaunch {
                continue;
            }
            if prev.app_id == event.app_id {
                continue;
            }

            let gap = event.timestamp.saturating_sub(prev.timestamp);
            if gap > self.sequence_gap_max {
                continue;
            }

            // Found a candidate sequence: prev.app_id -> event.app_id
            self.record_sequence(prev.app_id, event.app_id, gap);
            break;
        }
    }

    /// Record or update a detected sequence pair
    fn record_sequence(&mut self, first: u16, second: u16, gap: u64) {
        // Check if this sequence already exists
        for seq in self.sequences.iter_mut() {
            if seq.first_app == first && seq.second_app == second {
                seq.occurrences = seq.occurrences.saturating_add(1);
                seq.last_seen = self.total_events;
                // Running average of gap
                let old = seq.avg_gap_ticks;
                seq.avg_gap_ticks =
                    (old * (seq.occurrences as u64 - 1) + gap) / (seq.occurrences as u64);
                // Confidence grows with occurrences, capped at 1.0
                let raw = q16_div(
                    (seq.occurrences as i32) * Q16_ONE / 1,
                    (seq.occurrences as i32) + 10,
                );
                seq.confidence = core::cmp::min(raw, Q16_ONE);
                return;
            }
        }

        // New sequence
        if self.sequences.len() < MAX_SEQUENCES {
            self.sequences.push(ActionSequence {
                first_app: first,
                second_app: second,
                avg_gap_ticks: gap,
                occurrences: 1,
                confidence: Q16_HUNDREDTH,
                last_seen: self.total_events,
            });
        }
    }

    /// Update the Markov transition matrix
    fn update_transitions(&mut self, event: &ObservedEvent) {
        if event.kind != EventKind::AppLaunch {
            return;
        }

        // Find the most recent preceding launch
        let history_len = self.event_history.len();
        if history_len == 0 {
            return;
        }

        let mut prev_app: Option<u16> = None;
        let search_start = if history_len > 4 { history_len - 4 } else { 0 };
        for i in (search_start..history_len).rev() {
            if self.event_history[i].kind == EventKind::AppLaunch {
                prev_app = Some(self.event_history[i].app_id);
                break;
            }
        }

        let prev = match prev_app {
            Some(p) => p,
            None => return,
        };

        // Find existing transition or create new one
        let mut found = false;
        let mut from_total: u32 = 0;

        // Count total transitions from prev
        for cell in self.transitions.iter() {
            if cell.from_app == prev {
                from_total += cell.count;
            }
        }

        // Update or insert
        for cell in self.transitions.iter_mut() {
            if cell.from_app == prev && cell.to_app == event.app_id {
                cell.count = cell.count.saturating_add(1);
                found = true;
                break;
            }
        }

        if !found && self.transitions.len() < TRANSITION_MATRIX_SIZE * TRANSITION_MATRIX_SIZE {
            self.transitions.push(TransitionCell {
                from_app: prev,
                to_app: event.app_id,
                probability: Q16_ZERO,
                count: 1,
            });
        }

        // Recompute probabilities for all transitions from prev
        from_total += 1; // include the one we just added/incremented
        for cell in self.transitions.iter_mut() {
            if cell.from_app == prev {
                cell.probability = q16_div(cell.count as i32, from_total as i32);
            }
        }
    }

    /// Apply decay to all statistics (call periodically, e.g., daily)
    pub fn apply_decay(&mut self) {
        let decay = self.global_decay_rate;

        // Decay hourly/daily frequencies
        for stats in self.app_stats.iter_mut() {
            for h in 0..TIME_BUCKETS {
                stats.hourly_freq[h] = q16_mul(stats.hourly_freq[h], decay);
            }
            for d in 0..DAYS_OF_WEEK {
                stats.daily_freq[d] = q16_mul(stats.daily_freq[d], decay);
            }
            stats.momentum = q16_mul(stats.momentum, decay);
        }

        // Decay sequence confidence
        for seq in self.sequences.iter_mut() {
            seq.confidence = q16_mul(seq.confidence, decay);
        }

        // Prune sequences with near-zero confidence
        self.sequences.retain(|s| s.confidence > Q16_HUNDREDTH);
    }

    /// Predict the next app given the current app, using Markov transitions
    pub fn predict_next_app(&self, current_app: u16) -> Option<(u16, i32)> {
        let mut best_app: u16 = 0;
        let mut best_prob: i32 = Q16_ZERO;

        for cell in &self.transitions {
            if cell.from_app == current_app && cell.probability > best_prob {
                best_prob = cell.probability;
                best_app = cell.to_app;
            }
        }

        if best_prob > Q16_TENTH {
            Some((best_app, best_prob))
        } else {
            None
        }
    }

    /// Get the top N most-used apps for a given hour and day
    pub fn top_apps_for_time(&self, hour: u8, day: u8, max_results: usize) -> Vec<(u16, i32)> {
        let h = (hour as usize) % TIME_BUCKETS;
        let d = (day as usize) % DAYS_OF_WEEK;

        let mut scored: Vec<(u16, i32)> = Vec::new();
        for stats in &self.app_stats {
            // Combined score: 60% hourly + 40% daily
            let hour_score = q16_mul(stats.hourly_freq[h], 39322); // 0.6
            let day_score = q16_mul(stats.daily_freq[d], 26214); // 0.4
            let combined = hour_score + day_score;
            if combined > Q16_HUNDREDTH {
                scored.push((stats.app_id, combined));
            }
        }

        // Sort descending by score (simple insertion sort for small N)
        for i in 1..scored.len() {
            let mut j = i;
            while j > 0 && scored[j].1 > scored[j - 1].1 {
                scored.swap(j, j - 1);
                j -= 1;
            }
        }

        scored.truncate(max_results);
        scored
    }

    /// Get high-confidence action sequences
    pub fn get_strong_sequences(&self, min_confidence: i32) -> Vec<(u16, u16, i32)> {
        let mut results = Vec::new();
        for seq in &self.sequences {
            if seq.confidence >= min_confidence && seq.occurrences >= self.min_sequence_count {
                results.push((seq.first_app, seq.second_app, seq.confidence));
            }
        }
        results
    }

    /// Compute a frequency score for a specific app at a specific time
    pub fn app_relevance(&self, app_id: u16, hour: u8, day: u8) -> i32 {
        let h = (hour as usize) % TIME_BUCKETS;
        let d = (day as usize) % DAYS_OF_WEEK;

        for stats in &self.app_stats {
            if stats.app_id == app_id {
                let hour_weight = stats.hourly_freq[h];
                let day_weight = stats.daily_freq[d];
                let momentum_bonus = q16_mul(stats.momentum, Q16_TENTH);
                return q16_mul(hour_weight + day_weight, Q16_HALF) + momentum_bonus;
            }
        }
        Q16_ZERO
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static PATTERN_ENGINE: Mutex<Option<PatternEngine>> = Mutex::new(None);

/// Initialize the pattern detection subsystem
pub fn init() {
    let mut guard = PATTERN_ENGINE.lock();
    *guard = Some(PatternEngine::new());
    serial_println!("    [learning] Pattern engine initialized");
}

/// Record a user event
pub fn record(kind: EventKind, app_id: u16, timestamp: u64, hour: u8, day: u8, ctx: u32) {
    let mut guard = PATTERN_ENGINE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.record_event(ObservedEvent {
            kind,
            app_id,
            timestamp,
            hour,
            day_of_week: day,
            context_hash: ctx,
        });
    }
}

/// Predict the next app from the current one
pub fn predict_next(current_app: u16) -> Option<(u16, i32)> {
    let guard = PATTERN_ENGINE.lock();
    if let Some(engine) = guard.as_ref() {
        engine.predict_next_app(current_app)
    } else {
        None
    }
}

/// Get top apps for a given time
pub fn top_apps(hour: u8, day: u8, count: usize) -> Vec<(u16, i32)> {
    let guard = PATTERN_ENGINE.lock();
    if let Some(engine) = guard.as_ref() {
        engine.top_apps_for_time(hour, day, count)
    } else {
        Vec::new()
    }
}

/// Run periodic decay on all pattern data
pub fn decay() {
    let mut guard = PATTERN_ENGINE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.apply_decay();
    }
}

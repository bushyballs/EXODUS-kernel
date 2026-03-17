use crate::sync::Mutex;
/// Feedback loop for Genesis learning subsystem
///
/// Collects and processes both explicit and implicit feedback signals
/// to continuously refine the learning model:
///   - Explicit corrections: user says "no, I wanted X not Y"
///   - Implicit signals: dwell time, scroll depth, undo frequency
///   - Reinforcement: reward/penalize predictions based on outcomes
///   - Feedback aggregation: combine multiple signals into confidence updates
///   - A/B observation: track which adaptations perform better
///   - Trust calibration: adjust how much the system trusts itself
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
const Q16_NEG_ONE: i32 = -65536;

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

/// Absolute value for Q16
fn q16_abs(v: i32) -> i32 {
    if v < 0 {
        -v
    } else {
        v
    }
}

// ── Configuration ──────────────────────────────────────────────────────────

const MAX_FEEDBACK_LOG: usize = 512;
const MAX_CORRECTIONS: usize = 128;
const MAX_REINFORCEMENT_RULES: usize = 64;
const MAX_OBSERVATIONS: usize = 32;
const TRUST_WINDOW: usize = 50; // evaluate trust over last N feedback events

// ── Types ──────────────────────────────────────────────────────────────────

/// The type of feedback signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackType {
    ExplicitCorrection, // user explicitly corrected a prediction
    ExplicitApproval,   // user explicitly confirmed a prediction
    ImplicitDwell,      // long dwell = interest, short dwell = miss
    ImplicitScroll,     // deep scroll = engagement
    ImplicitUndo,       // undo = mistake or wrong prediction
    ImplicitIgnore,     // suggestion shown but not acted on
    ImplicitAccept,     // suggestion acted on without explicit approval
    SystemOutcome,      // prediction was verifiable (preload hit/miss)
}

/// Polarity of the feedback: positive, negative, or neutral
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackPolarity {
    Positive,
    Negative,
    Neutral,
}

/// A single feedback event
pub struct FeedbackEvent {
    pub feedback_type: FeedbackType,
    pub polarity: FeedbackPolarity,
    pub target_id: u32, // what the feedback is about
    pub magnitude: i32, // Q16 [-1..1] strength of signal
    pub timestamp: u64,
    pub context_hash: u32, // hash of surrounding state
    pub processed: bool,
}

/// An explicit user correction
pub struct Correction {
    pub prediction_id: u32,   // which prediction was wrong
    pub predicted_value: u32, // what we predicted
    pub correct_value: u32,   // what the user wanted
    pub timestamp: u64,
    pub context_hash: u32,
    pub applied: bool, // have we applied this correction?
}

/// A reinforcement rule: maps outcomes to reward/penalty
pub struct ReinforcementRule {
    pub rule_id: u16,
    pub target_type: u8,         // 0=app, 1=action, 2=setting, 3=suggestion
    pub positive_reward: i32,    // Q16 how much to boost on positive outcome
    pub negative_penalty: i32,   // Q16 how much to penalize on negative outcome
    pub decay_rate: i32,         // Q16 per-cycle decay of accumulated reward
    pub accumulated_reward: i32, // Q16 current reward balance
    pub total_positive: u32,
    pub total_negative: u32,
    pub active: bool,
}

impl ReinforcementRule {
    fn apply_outcome(&mut self, positive: bool) {
        if positive {
            self.accumulated_reward += self.positive_reward;
            self.total_positive = self.total_positive.saturating_add(1);
        } else {
            self.accumulated_reward -= self.negative_penalty;
            self.total_negative = self.total_negative.saturating_add(1);
        }
        // Clamp to [-2.0, 2.0]
        self.accumulated_reward = q16_clamp(
            self.accumulated_reward,
            -131072, // -2.0
            131072,  // 2.0
        );
    }

    fn decay(&mut self) {
        self.accumulated_reward = q16_mul(self.accumulated_reward, self.decay_rate);
        if q16_abs(self.accumulated_reward) < Q16_HUNDREDTH {
            self.accumulated_reward = Q16_ZERO;
        }
    }

    /// Ratio of positive to total outcomes (Q16)
    fn success_rate(&self) -> i32 {
        let total = self.total_positive + self.total_negative;
        if total == 0 {
            return Q16_HALF;
        }
        q16_div(self.total_positive as i32, total as i32)
    }
}

/// An A/B observation: compare two strategies
pub struct ABObservation {
    pub observation_id: u16,
    pub strategy_a_id: u16,
    pub strategy_b_id: u16,
    pub a_positive: u32,
    pub a_negative: u32,
    pub b_positive: u32,
    pub b_negative: u32,
    pub min_samples: u32,    // minimum samples before making a decision
    pub winner: Option<u16>, // determined winner (None if inconclusive)
    pub active: bool,
}

impl ABObservation {
    fn a_rate(&self) -> i32 {
        let total = self.a_positive + self.a_negative;
        if total == 0 {
            return Q16_HALF;
        }
        q16_div(self.a_positive as i32, total as i32)
    }

    fn b_rate(&self) -> i32 {
        let total = self.b_positive + self.b_negative;
        if total == 0 {
            return Q16_HALF;
        }
        q16_div(self.b_positive as i32, total as i32)
    }

    /// Evaluate if we have a statistically significant winner
    fn evaluate(&mut self) {
        let total_a = self.a_positive + self.a_negative;
        let total_b = self.b_positive + self.b_negative;

        if total_a < self.min_samples || total_b < self.min_samples {
            return; // not enough data
        }

        let rate_a = self.a_rate();
        let rate_b = self.b_rate();
        let diff = q16_abs(rate_a - rate_b);

        // Require at least 10% difference (6554 in Q16) to declare winner
        if diff > Q16_TENTH {
            self.winner = if rate_a > rate_b {
                Some(self.strategy_a_id)
            } else {
                Some(self.strategy_b_id)
            };
            self.active = false;
        }
    }
}

/// The main feedback processor
pub struct FeedbackProcessor {
    pub enabled: bool,
    pub events: Vec<FeedbackEvent>,
    pub event_write_idx: usize,
    pub corrections: Vec<Correction>,
    pub reinforcement_rules: Vec<ReinforcementRule>,
    pub observations: Vec<ABObservation>,
    pub next_observation_id: u16,

    // Trust calibration
    pub system_trust: i32, // Q16 [0..1] how much the system trusts its predictions
    pub trust_history: Vec<bool>, // recent prediction outcomes (true=correct)
    pub trust_write_idx: usize,
    pub trust_floor: i32,   // Q16 minimum trust level
    pub trust_ceiling: i32, // Q16 maximum trust level

    // Aggregated stats
    pub total_positive: u64,
    pub total_negative: u64,
    pub total_neutral: u64,
    pub correction_count: u64,
}

impl FeedbackProcessor {
    const fn new() -> Self {
        FeedbackProcessor {
            enabled: true,
            events: Vec::new(),
            event_write_idx: 0,
            corrections: Vec::new(),
            reinforcement_rules: Vec::new(),
            observations: Vec::new(),
            next_observation_id: 1,
            system_trust: Q16_HALF,
            trust_history: Vec::new(),
            trust_write_idx: 0,
            trust_floor: Q16_TENTH, // never go below 10% trust
            trust_ceiling: 58982,   // never go above 90% trust (0.9)
            total_positive: 0,
            total_negative: 0,
            total_neutral: 0,
            correction_count: 0,
        }
    }

    /// Record a feedback event
    pub fn record_feedback(
        &mut self,
        feedback_type: FeedbackType,
        polarity: FeedbackPolarity,
        target_id: u32,
        magnitude: i32,
        timestamp: u64,
        context_hash: u32,
    ) {
        if !self.enabled {
            return;
        }

        let event = FeedbackEvent {
            feedback_type,
            polarity,
            target_id,
            magnitude: q16_clamp(magnitude, Q16_NEG_ONE, Q16_ONE),
            timestamp,
            context_hash,
            processed: false,
        };

        // Update aggregate counts
        match polarity {
            FeedbackPolarity::Positive => {
                self.total_positive = self.total_positive.saturating_add(1)
            }
            FeedbackPolarity::Negative => {
                self.total_negative = self.total_negative.saturating_add(1)
            }
            FeedbackPolarity::Neutral => self.total_neutral = self.total_neutral.saturating_add(1),
        }

        // Store in ring buffer
        if self.events.len() < MAX_FEEDBACK_LOG {
            self.events.push(event);
        } else {
            let idx = self.event_write_idx % MAX_FEEDBACK_LOG;
            self.events[idx] = event;
        }
        self.event_write_idx += 1;

        // Update trust
        let is_positive = matches!(polarity, FeedbackPolarity::Positive);
        self.update_trust(is_positive);

        // Apply to matching reinforcement rules
        self.apply_reinforcement(target_id, is_positive);
    }

    /// Record an explicit correction
    pub fn record_correction(
        &mut self,
        prediction_id: u32,
        predicted: u32,
        correct: u32,
        timestamp: u64,
        context_hash: u32,
    ) {
        self.correction_count = self.correction_count.saturating_add(1);

        if self.corrections.len() < MAX_CORRECTIONS {
            self.corrections.push(Correction {
                prediction_id,
                predicted_value: predicted,
                correct_value: correct,
                timestamp,
                context_hash,
                applied: false,
            });
        } else {
            // Overwrite oldest applied correction
            for corr in self.corrections.iter_mut() {
                if corr.applied {
                    *corr = Correction {
                        prediction_id,
                        predicted_value: predicted,
                        correct_value: correct,
                        timestamp,
                        context_hash,
                        applied: false,
                    };
                    break;
                }
            }
        }

        // Strong negative signal on trust
        self.update_trust(false);
        self.update_trust(false); // double penalty for explicit correction
    }

    /// Update trust based on a single observation
    fn update_trust(&mut self, correct: bool) {
        // Add to trust history ring buffer
        if self.trust_history.len() < TRUST_WINDOW {
            self.trust_history.push(correct);
        } else {
            let idx = self.trust_write_idx % TRUST_WINDOW;
            self.trust_history[idx] = correct;
        }
        self.trust_write_idx += 1;

        // Recalculate trust from history
        if self.trust_history.is_empty() {
            return;
        }

        let mut correct_count: i32 = 0;
        let total = self.trust_history.len() as i32;
        for outcome in &self.trust_history {
            if *outcome {
                correct_count += 1;
            }
        }

        let raw_trust = q16_div(correct_count, total);
        self.system_trust = q16_clamp(raw_trust, self.trust_floor, self.trust_ceiling);
    }

    /// Apply reinforcement to matching rules
    fn apply_reinforcement(&mut self, target_id: u32, positive: bool) {
        let target_lower = (target_id & 0xFF) as u8;
        for rule in self.reinforcement_rules.iter_mut() {
            if !rule.active {
                continue;
            }
            // Match by target type (simplified: use lower byte of target_id)
            if rule.target_type == target_lower {
                rule.apply_outcome(positive);
            }
        }
    }

    /// Register a new reinforcement rule
    pub fn add_reinforcement_rule(
        &mut self,
        rule_id: u16,
        target_type: u8,
        reward: i32,
        penalty: i32,
        decay: i32,
    ) {
        if self.reinforcement_rules.len() >= MAX_REINFORCEMENT_RULES {
            return;
        }
        self.reinforcement_rules.push(ReinforcementRule {
            rule_id,
            target_type,
            positive_reward: q16_clamp(reward, Q16_ZERO, Q16_ONE),
            negative_penalty: q16_clamp(penalty, Q16_ZERO, Q16_ONE),
            decay_rate: q16_clamp(decay, Q16_HALF, Q16_ONE),
            accumulated_reward: Q16_ZERO,
            total_positive: 0,
            total_negative: 0,
            active: true,
        });
    }

    /// Start an A/B observation between two strategies
    pub fn start_ab_observation(
        &mut self,
        strategy_a: u16,
        strategy_b: u16,
        min_samples: u32,
    ) -> u16 {
        let id = self.next_observation_id;
        self.next_observation_id = self.next_observation_id.saturating_add(1);

        if self.observations.len() < MAX_OBSERVATIONS {
            self.observations.push(ABObservation {
                observation_id: id,
                strategy_a_id: strategy_a,
                strategy_b_id: strategy_b,
                a_positive: 0,
                a_negative: 0,
                b_positive: 0,
                b_negative: 0,
                min_samples,
                winner: None,
                active: true,
            });
        }
        id
    }

    /// Record an outcome for an A/B observation
    pub fn record_ab_outcome(&mut self, observation_id: u16, is_strategy_a: bool, positive: bool) {
        for obs in self.observations.iter_mut() {
            if obs.observation_id == observation_id && obs.active {
                if is_strategy_a {
                    if positive {
                        obs.a_positive = obs.a_positive.saturating_add(1);
                    } else {
                        obs.a_negative = obs.a_negative.saturating_add(1);
                    }
                } else {
                    if positive {
                        obs.b_positive = obs.b_positive.saturating_add(1);
                    } else {
                        obs.b_negative = obs.b_negative.saturating_add(1);
                    }
                }
                obs.evaluate();
                return;
            }
        }
    }

    /// Get the winner of an A/B observation (None if not concluded)
    pub fn ab_winner(&self, observation_id: u16) -> Option<u16> {
        for obs in &self.observations {
            if obs.observation_id == observation_id {
                return obs.winner;
            }
        }
        None
    }

    /// Get unapplied corrections for a given prediction context
    pub fn pending_corrections(&self, context_hash: u32) -> Vec<(u32, u32)> {
        let mut results = Vec::new();
        for corr in &self.corrections {
            if !corr.applied && corr.context_hash == context_hash {
                results.push((corr.predicted_value, corr.correct_value));
            }
        }
        results
    }

    /// Mark corrections as applied
    pub fn mark_corrections_applied(&mut self, context_hash: u32) {
        for corr in self.corrections.iter_mut() {
            if corr.context_hash == context_hash {
                corr.applied = true;
            }
        }
    }

    /// Process implicit feedback signals (dwell time, scroll depth)
    pub fn process_implicit_dwell(&mut self, target_id: u32, dwell_ms: u64, timestamp: u64) {
        // Dwell time interpretation:
        //   < 500ms = probably wrong content (negative)
        //   500ms - 3000ms = neutral
        //   > 3000ms = engaged (positive)
        let (polarity, magnitude) = if dwell_ms < 500 {
            (FeedbackPolarity::Negative, -Q16_QUARTER)
        } else if dwell_ms < 3000 {
            (FeedbackPolarity::Neutral, Q16_ZERO)
        } else if dwell_ms < 10000 {
            (FeedbackPolarity::Positive, Q16_QUARTER)
        } else {
            (FeedbackPolarity::Positive, Q16_HALF)
        };

        self.record_feedback(
            FeedbackType::ImplicitDwell,
            polarity,
            target_id,
            magnitude,
            timestamp,
            0,
        );
    }

    /// Process an undo signal (always negative)
    pub fn process_undo(&mut self, target_id: u32, timestamp: u64) {
        self.record_feedback(
            FeedbackType::ImplicitUndo,
            FeedbackPolarity::Negative,
            target_id,
            -Q16_HALF,
            timestamp,
            0,
        );
    }

    /// Run periodic maintenance: decay rules, prune old events
    pub fn maintenance(&mut self) {
        // Decay reinforcement rules
        for rule in self.reinforcement_rules.iter_mut() {
            if rule.active {
                rule.decay();
            }
        }

        // Prune completed A/B observations
        self.observations.retain(|o| o.active);

        // Prune applied corrections older than some threshold
        self.corrections.retain(|c| !c.applied);
    }

    /// Compute overall system health: Q16 score based on feedback balance
    pub fn system_health(&self) -> i32 {
        let total = self.total_positive + self.total_negative;
        if total == 0 {
            return Q16_HALF;
        }

        let pos_rate = q16_div(self.total_positive as i32, total as i32);

        // Weight by trust
        q16_mul(pos_rate, self.system_trust) + q16_mul(Q16_HALF, Q16_ONE - self.system_trust)
    }

    /// Get the confidence adjustment factor for predictions
    /// This scales raw prediction confidence by system trust
    pub fn confidence_scale(&self) -> i32 {
        self.system_trust
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static FEEDBACK: Mutex<Option<FeedbackProcessor>> = Mutex::new(None);

pub fn init() {
    let mut guard = FEEDBACK.lock();
    let mut processor = FeedbackProcessor::new();

    // Set up default reinforcement rules
    processor.add_reinforcement_rule(1, 0, Q16_TENTH, Q16_TENTH, 62259); // app predictions
    processor.add_reinforcement_rule(2, 1, Q16_QUARTER, Q16_TENTH, 62259); // action suggestions
    processor.add_reinforcement_rule(3, 2, Q16_TENTH, Q16_QUARTER, 64880); // setting changes
    processor.add_reinforcement_rule(4, 3, Q16_QUARTER, Q16_QUARTER, 62259); // general suggestions

    *guard = Some(processor);
    serial_println!("    [learning] Feedback processor initialized");
}

/// Record positive feedback
pub fn positive(target_id: u32, timestamp: u64) {
    let mut guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_mut() {
        proc.record_feedback(
            FeedbackType::ImplicitAccept,
            FeedbackPolarity::Positive,
            target_id,
            Q16_QUARTER,
            timestamp,
            0,
        );
    }
}

/// Record negative feedback
pub fn negative(target_id: u32, timestamp: u64) {
    let mut guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_mut() {
        proc.record_feedback(
            FeedbackType::ImplicitIgnore,
            FeedbackPolarity::Negative,
            target_id,
            -Q16_QUARTER,
            timestamp,
            0,
        );
    }
}

/// Record an explicit correction
pub fn correct(prediction_id: u32, predicted: u32, correct_val: u32, timestamp: u64) {
    let mut guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_mut() {
        proc.record_correction(prediction_id, predicted, correct_val, timestamp, 0);
    }
}

/// Record dwell time feedback
pub fn dwell(target_id: u32, dwell_ms: u64, timestamp: u64) {
    let mut guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_mut() {
        proc.process_implicit_dwell(target_id, dwell_ms, timestamp);
    }
}

/// Get current system trust level (Q16)
pub fn trust() -> i32 {
    let guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_ref() {
        proc.system_trust
    } else {
        Q16_HALF
    }
}

/// Get the confidence scaling factor
pub fn confidence_scale() -> i32 {
    let guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_ref() {
        proc.confidence_scale()
    } else {
        Q16_HALF
    }
}

/// Run periodic maintenance
pub fn maintenance() {
    let mut guard = FEEDBACK.lock();
    if let Some(proc) = guard.as_mut() {
        proc.maintenance();
    }
}

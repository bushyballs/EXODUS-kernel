use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
/// Reinforcement learning from user feedback
///
/// Part of the AIOS AI layer. Implements a reinforcement-style feedback loop
/// that tracks action-reward pairs, updates action weights via exponential
/// moving average, and selects best actions based on accumulated scores.
///
/// Actions are identified by string keys. Each action accumulates a weighted
/// score from feedback signals. The system supports multiple contexts
/// (e.g., different task types) with independent action weights.
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A single feedback signal from the user
pub struct FeedbackSignal {
    pub response_id: u64,
    pub score: i8, // -1 negative, 0 neutral, +1 positive
    pub action_key: String,
    pub context: String,
    pub detail_score: f32,
}

impl FeedbackSignal {
    /// Create a simple feedback signal
    pub fn new(response_id: u64, score: i8, action_key: &str) -> Self {
        FeedbackSignal {
            response_id,
            score,
            action_key: String::from(action_key),
            context: String::from("default"),
            detail_score: score as f32,
        }
    }

    /// Create a detailed feedback signal with float score and context
    pub fn detailed(response_id: u64, detail_score: f32, action_key: &str, context: &str) -> Self {
        let score = if detail_score > 0.3 {
            1
        } else if detail_score < -0.3 {
            -1
        } else {
            0
        };
        FeedbackSignal {
            response_id,
            score,
            action_key: String::from(action_key),
            context: String::from(context),
            detail_score,
        }
    }
}

/// Tracked state for a single action
struct ActionState {
    /// Exponential moving average of reward
    ema_reward: f32,
    /// Total number of times this action received feedback
    feedback_count: u64,
    /// Total cumulative reward (unsmoothed)
    cumulative_reward: f32,
    /// Number of times this action was selected/recommended
    selection_count: u64,
    /// Last feedback score received
    last_score: f32,
}

impl ActionState {
    fn new() -> Self {
        ActionState {
            ema_reward: 0.0,
            feedback_count: 0,
            cumulative_reward: 0.0,
            selection_count: 0,
            last_score: 0.0,
        }
    }

    /// Update EMA with a new reward signal
    fn update(&mut self, reward: f32, alpha: f32) {
        self.feedback_count = self.feedback_count.saturating_add(1);
        self.cumulative_reward += reward;
        self.last_score = reward;
        // Exponential moving average: EMA = alpha * new + (1 - alpha) * old
        self.ema_reward = alpha * reward + (1.0 - alpha) * self.ema_reward;
    }

    /// Score used for ranking: EMA with exploration bonus (UCB-like)
    fn selection_score(&self) -> f32 {
        // UCB1-inspired: reward + sqrt(2 * ln(total) / count)
        // Simplified since we don't have ln easily: use 1/sqrt(count) bonus
        let exploration = if self.feedback_count == 0 {
            1.0 // High exploration bonus for untried actions
        } else {
            1.0 / sqrt_f32(self.feedback_count as f32)
        };
        self.ema_reward + 0.5 * exploration
    }
}

/// Per-context action weights
struct ContextWeights {
    actions: BTreeMap<String, ActionState>,
}

impl ContextWeights {
    fn new() -> Self {
        ContextWeights {
            actions: BTreeMap::new(),
        }
    }

    fn get_or_create(&mut self, action_key: &str) -> &mut ActionState {
        // Use entry API to avoid a double-lookup and eliminate the .unwrap() —
        // or_insert_with guarantees the key is present before we return the mutable ref.
        self.actions
            .entry(String::from(action_key))
            .or_insert_with(ActionState::new)
    }
}

/// Collects feedback and adjusts model behavior
pub struct FeedbackLoop {
    pub signals: Vec<FeedbackSignal>,
    pub learning_rate: f32,
    /// Per-context action weights
    contexts: BTreeMap<String, ContextWeights>,
    /// Maximum signals to retain in history
    max_history: usize,
    /// Total feedback signals processed
    total_signals: u64,
    /// Running average reward across all signals
    global_avg_reward: f32,
    /// Decay factor applied to old signals during pruning
    decay_factor: f32,
    /// Next response ID to assign
    next_response_id: u64,
}

impl FeedbackLoop {
    pub fn new() -> Self {
        FeedbackLoop {
            signals: Vec::new(),
            learning_rate: 0.3,
            contexts: BTreeMap::new(),
            max_history: 1024,
            total_signals: 0,
            global_avg_reward: 0.0,
            decay_factor: 0.95,
            next_response_id: 1,
        }
    }

    /// Create with custom learning rate
    pub fn with_learning_rate(lr: f32) -> Self {
        let mut fl = Self::new();
        fl.learning_rate = lr.max(0.01).min(1.0);
        fl
    }

    /// Record a feedback signal and update action weights
    pub fn record(&mut self, signal: FeedbackSignal) {
        let ctx_key = signal.context.clone();
        let action_key = signal.action_key.clone();
        let reward = signal.detail_score;

        // Update per-context action state
        let ctx = self
            .contexts
            .entry(ctx_key)
            .or_insert_with(ContextWeights::new);
        let action = ctx.get_or_create(&action_key);
        action.update(reward, self.learning_rate);

        // Update global statistics
        self.total_signals = self.total_signals.saturating_add(1);
        let n = self.total_signals as f32;
        self.global_avg_reward = self.global_avg_reward * ((n - 1.0) / n) + reward / n;

        // Store signal in history
        self.signals.push(signal);

        // Prune old signals if over capacity
        if self.signals.len() > self.max_history {
            let excess = self.signals.len() - self.max_history;
            self.signals.drain(..excess);
        }
    }

    /// Record a simple thumbs-up/down signal
    pub fn record_simple(&mut self, action_key: &str, positive: bool) {
        let score = if positive { 1 } else { -1 };
        let id = self.next_response_id;
        self.next_response_id = self.next_response_id.saturating_add(1);
        self.record(FeedbackSignal::new(id, score, action_key));
    }

    /// Compute the current aggregate reward
    pub fn compute_reward(&self) -> f32 {
        if self.signals.is_empty() {
            return 0.0;
        }

        // Weighted recent average: more recent signals count more
        let mut weighted_sum = 0.0f32;
        let mut weight_total = 0.0f32;
        let len = self.signals.len();

        for (i, signal) in self.signals.iter().enumerate() {
            // Exponential decay: recent signals have weight ~1, old signals decay
            let age = (len - 1 - i) as f32;
            let weight = pow_f32(self.decay_factor, age);
            weighted_sum += signal.detail_score * weight;
            weight_total += weight;
        }

        if weight_total > 0.0 {
            weighted_sum / weight_total
        } else {
            0.0
        }
    }

    /// Get the best action for a given context, based on accumulated rewards
    pub fn best_action(&self, context: &str) -> Option<(String, f32)> {
        let ctx = self.contexts.get(context)?;
        let mut best: Option<(&String, f32)> = None;

        for (key, state) in &ctx.actions {
            let score = state.selection_score();
            match &best {
                Some((_, best_score)) => {
                    if score > *best_score {
                        best = Some((key, score));
                    }
                }
                None => {
                    best = Some((key, score));
                }
            }
        }

        best.map(|(k, s)| (k.clone(), s))
    }

    /// Get ranked actions for a given context
    pub fn ranked_actions(&self, context: &str) -> Vec<(String, f32)> {
        match self.contexts.get(context) {
            Some(ctx) => {
                let mut actions: Vec<(String, f32)> = ctx
                    .actions
                    .iter()
                    .map(|(k, v)| (k.clone(), v.selection_score()))
                    .collect();
                actions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
                actions
            }
            None => Vec::new(),
        }
    }

    /// Mark that an action was selected (for exploration tracking)
    pub fn mark_selected(&mut self, context: &str, action_key: &str) {
        let ctx = self
            .contexts
            .entry(String::from(context))
            .or_insert_with(ContextWeights::new);
        let action = ctx.get_or_create(action_key);
        action.selection_count += 1;
    }

    /// Get the EMA reward for a specific action in a context
    pub fn action_reward(&self, context: &str, action_key: &str) -> Option<f32> {
        self.contexts
            .get(context)
            .and_then(|ctx| ctx.actions.get(action_key))
            .map(|a| a.ema_reward)
    }

    /// Get total feedback count for an action
    pub fn action_feedback_count(&self, context: &str, action_key: &str) -> u64 {
        self.contexts
            .get(context)
            .and_then(|ctx| ctx.actions.get(action_key))
            .map(|a| a.feedback_count)
            .unwrap_or(0)
    }

    /// Get global average reward
    pub fn global_reward(&self) -> f32 {
        self.global_avg_reward
    }

    /// Total feedback signals processed
    pub fn total_signals(&self) -> u64 {
        self.total_signals
    }

    /// Number of unique contexts tracked
    pub fn context_count(&self) -> usize {
        self.contexts.len()
    }

    /// Number of unique actions tracked across all contexts
    pub fn total_actions(&self) -> usize {
        self.contexts.values().map(|ctx| ctx.actions.len()).sum()
    }

    /// Apply decay to all action weights (call periodically to forget old patterns)
    pub fn apply_decay(&mut self) {
        for ctx in self.contexts.values_mut() {
            for action in ctx.actions.values_mut() {
                action.ema_reward *= self.decay_factor;
                action.cumulative_reward *= self.decay_factor;
            }
        }
    }

    /// Reset all feedback data
    pub fn reset(&mut self) {
        self.signals.clear();
        self.contexts.clear();
        self.total_signals = 0;
        self.global_avg_reward = 0.0;
        self.next_response_id = 1;
    }

    /// Get a summary of recent feedback performance
    pub fn performance_summary(&self) -> String {
        let positive = self.signals.iter().filter(|s| s.score > 0).count();
        let negative = self.signals.iter().filter(|s| s.score < 0).count();
        let neutral = self.signals.iter().filter(|s| s.score == 0).count();
        format!(
            "Feedback: {} total ({} pos, {} neg, {} neutral), avg reward: {:.3}, contexts: {}",
            self.total_signals,
            positive,
            negative,
            neutral,
            self.global_avg_reward,
            self.contexts.len()
        )
    }
}

// ---------------------------------------------------------------------------
// Math helpers
// ---------------------------------------------------------------------------

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x / 2.0;
    for _ in 0..32 {
        let next = 0.5 * (guess + x / guess);
        if (next - guess).abs() < 1e-7 {
            break;
        }
        guess = next;
    }
    guess
}

fn pow_f32(base: f32, exp: f32) -> f32 {
    if exp == 0.0 {
        return 1.0;
    }
    // For integer-like exponents, use iterative multiplication
    let n = exp as u32;
    let mut result = 1.0f32;
    for _ in 0..n {
        result *= base;
    }
    result
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FEEDBACK: Mutex<Option<FeedbackLoop>> = Mutex::new(None);

pub fn init() {
    *FEEDBACK.lock() = Some(FeedbackLoop::new());
    crate::serial_println!("    [feedback_loop] Feedback loop ready (EMA alpha=0.3, decay=0.95)");
}

/// Record a feedback signal
pub fn record(signal: FeedbackSignal) {
    if let Some(fl) = FEEDBACK.lock().as_mut() {
        fl.record(signal);
    }
}

/// Get the current reward
pub fn compute_reward() -> f32 {
    FEEDBACK
        .lock()
        .as_ref()
        .map(|fl| fl.compute_reward())
        .unwrap_or(0.0)
}

/// Get the best action for a context
pub fn best_action(context: &str) -> Option<(String, f32)> {
    FEEDBACK
        .lock()
        .as_ref()
        .and_then(|fl| fl.best_action(context))
}

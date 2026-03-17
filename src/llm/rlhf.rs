use crate::sync::Mutex;
/// RLHF — Reinforcement Learning from Human Feedback
///
/// Makes the Hoags AI align to YOUR preferences.
/// No corporate filters, no external alignment — you decide
/// what the AI should and shouldn't do.
///
///   - Preference pairs (chosen vs rejected responses)
///   - DPO (Direct Preference Optimization) — no reward model needed
///   - User feedback collection
///   - Continuous learning from interactions
///   - Custom behavior rules (your rules, not anyone else's)
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

#[derive(Clone, Copy, PartialEq)]
pub enum FeedbackType {
    ThumbsUp,
    ThumbsDown,
    Preference, // A over B
    Correction, // User edited the response
    Rating,     // 1-5 scale
}

#[derive(Clone, Copy)]
struct PreferencePair {
    prompt_hash: u64,
    chosen_hash: u64,      // The preferred response
    rejected_hash: u64,    // The worse response
    chosen_logprob: Q16,   // Log probability of chosen under policy
    rejected_logprob: Q16, // Log probability of rejected
    timestamp: u64,
    weight: Q16, // How much to weight this example
}

#[derive(Clone, Copy)]
struct UserFeedback {
    response_hash: u64,
    feedback_type: FeedbackType,
    rating: u8, // 0-5 (0 = N/A)
    timestamp: u64,
}

#[derive(Clone, Copy)]
struct BehaviorRule {
    id: u32,
    description_hash: u64,
    priority: u8,
    enabled: bool,
    // What this rule controls
    applies_to_hash: u64, // Topic/domain hash (0 = all)
    action: RuleAction,
}

#[derive(Clone, Copy, PartialEq)]
pub enum RuleAction {
    Encourage,  // Boost probability of this behavior
    Allow,      // Default — no modification
    Discourage, // Reduce probability (but don't block)
}

struct RlhfEngine {
    preferences: Vec<PreferencePair>,
    feedback: Vec<UserFeedback>,
    rules: Vec<BehaviorRule>,
    // DPO parameters
    beta: Q16, // DPO temperature (controls how strongly to optimize)
    // Stats
    total_feedback: u64,
    total_preferences: u32,
    total_corrections: u32,
    positive_ratio: u32, // x100 (percentage of thumbs up)
    next_rule_id: u32,
    // Learning
    dpo_steps_done: u32,
    last_dpo_loss: Q16,
}

static RLHF: Mutex<Option<RlhfEngine>> = Mutex::new(None);

impl RlhfEngine {
    fn new() -> Self {
        RlhfEngine {
            preferences: Vec::new(),
            feedback: Vec::new(),
            rules: Vec::new(),
            beta: q16_from_int(1) / 10, // 0.1
            total_feedback: 0,
            total_preferences: 0,
            total_corrections: 0,
            positive_ratio: 50,
            next_rule_id: 1,
            dpo_steps_done: 0,
            last_dpo_loss: 0,
        }
    }

    /// Record user preference: they chose response A over B
    fn add_preference(
        &mut self,
        prompt: u64,
        chosen: u64,
        rejected: u64,
        chosen_lp: Q16,
        rejected_lp: Q16,
        timestamp: u64,
    ) {
        self.preferences.push(PreferencePair {
            prompt_hash: prompt,
            chosen_hash: chosen,
            rejected_hash: rejected,
            chosen_logprob: chosen_lp,
            rejected_logprob: rejected_lp,
            timestamp,
            weight: q16_from_int(1),
        });
        self.total_preferences = self.total_preferences.saturating_add(1);
    }

    /// Record simple feedback
    fn add_feedback(
        &mut self,
        response_hash: u64,
        fb_type: FeedbackType,
        rating: u8,
        timestamp: u64,
    ) {
        self.feedback.push(UserFeedback {
            response_hash,
            feedback_type: fb_type,
            rating,
            timestamp,
        });
        self.total_feedback = self.total_feedback.saturating_add(1);

        // Update positive ratio
        if fb_type == FeedbackType::ThumbsUp {
            self.positive_ratio = (self.positive_ratio * 99 + 100) / 100;
        } else if fb_type == FeedbackType::ThumbsDown {
            self.positive_ratio = (self.positive_ratio * 99) / 100;
        }
        if fb_type == FeedbackType::Correction {
            self.total_corrections = self.total_corrections.saturating_add(1);
        }
    }

    /// DPO loss: -log(sigmoid(β * (log π(chosen) - log π(rejected))))
    /// Returns gradient signal for model weights
    fn dpo_loss(&self, chosen_lp: Q16, rejected_lp: Q16) -> Q16 {
        let diff = chosen_lp - rejected_lp;
        let scaled = q16_mul(self.beta, diff);

        // sigmoid approximation
        let sigmoid = if scaled > q16_from_int(4) {
            q16_from_int(1)
        } else if scaled < q16_from_int(-4) {
            0
        } else {
            (q16_from_int(1) >> 1) + (scaled >> 3)
        };

        // -log(sigmoid) approximation: for sigmoid near 1, loss near 0
        // For sigmoid near 0, loss is large
        let loss = q16_from_int(1) - sigmoid; // Simplified
        loss
    }

    /// Run one DPO training step on accumulated preferences
    fn dpo_step(&mut self) -> Q16 {
        if self.preferences.is_empty() {
            return 0;
        }

        let mut total_loss: i64 = 0;
        let n = self.preferences.len();

        for pref in &self.preferences {
            let loss = self.dpo_loss(pref.chosen_logprob, pref.rejected_logprob);
            total_loss += q16_mul(loss, pref.weight) as i64;
        }

        let avg_loss = (total_loss / n as i64) as Q16;
        self.dpo_steps_done = self.dpo_steps_done.saturating_add(1);
        self.last_dpo_loss = avg_loss;
        avg_loss
    }

    /// Add a custom behavior rule
    fn add_rule(&mut self, description: u64, action: RuleAction, priority: u8, domain: u64) -> u32 {
        let id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);
        self.rules.push(BehaviorRule {
            id,
            description_hash: description,
            priority,
            enabled: true,
            applies_to_hash: domain,
            action,
        });
        id
    }

    /// Check rules for a given topic
    fn check_rules(&self, topic_hash: u64) -> RuleAction {
        let mut best_action = RuleAction::Allow;
        let mut best_priority = 0u8;

        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            if rule.applies_to_hash == 0 || rule.applies_to_hash == topic_hash {
                if rule.priority > best_priority {
                    best_priority = rule.priority;
                    best_action = rule.action;
                }
            }
        }
        best_action
    }

    fn get_stats(&self) -> (u64, u32, u32, u32) {
        (
            self.total_feedback,
            self.total_preferences,
            self.positive_ratio,
            self.dpo_steps_done,
        )
    }
}

pub fn init() {
    let mut r = RLHF.lock();
    *r = Some(RlhfEngine::new());
    serial_println!(
        "    RLHF: DPO preference learning, user feedback, custom behavior rules ready"
    );
}

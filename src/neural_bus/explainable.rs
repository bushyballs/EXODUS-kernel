use super::*;
use crate::{serial_print, serial_println};
use alloc::vec::Vec;
use alloc::string::String;
use alloc::collections::BTreeMap;

/// Types of AI decisions that can be explained
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecisionType {
    LayoutChange,
    ColorAdaptation,
    AppPreload,
    NotificationFilter,
    BatteryOptimization,
    SecurityAction,
    ShortcutFired,
    EmotionResponse,
    CacheEviction,
    PriorityAdjustment,
}

impl DecisionType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionType::LayoutChange => "LayoutChange",
            DecisionType::ColorAdaptation => "ColorAdaptation",
            DecisionType::AppPreload => "AppPreload",
            DecisionType::NotificationFilter => "NotificationFilter",
            DecisionType::BatteryOptimization => "BatteryOptimization",
            DecisionType::SecurityAction => "SecurityAction",
            DecisionType::ShortcutFired => "ShortcutFired",
            DecisionType::EmotionResponse => "EmotionResponse",
            DecisionType::CacheEviction => "CacheEviction",
            DecisionType::PriorityAdjustment => "PriorityAdjustment",
        }
    }
}

/// Outcome of a user-provided decision feedback
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Accepted,
    Rejected,
    Deferred,
    Failed,
    NoFeedback,
}

/// Explanation verbosity level for user-facing output
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExplanationLevel {
    Silent,    // No explanations
    Summary,   // One-liner
    Detailed,  // Multi-sentence with reasoning
    Debug,     // Full technical details
}

/// Single contributing factor to a decision (e.g., "time_of_day", "battery_level")
#[derive(Clone, Debug)]
pub struct DecisionFactor {
    pub name: String,
    pub weight: Q16,      // How much this factor influenced the decision (0.0 - 1.0)
    pub value: Q16,       // The actual measured/computed value
    pub description: String,
}

impl DecisionFactor {
    pub fn new(name: &str, weight: Q16, value: Q16, description: &str) -> Self {
        DecisionFactor {
            name: String::from(name),
            weight,
            value,
            description: String::from(description),
        }
    }
}

/// An AI decision with full transparency data
#[derive(Clone, Debug)]
pub struct AiDecision {
    pub id: u64,
    pub timestamp: u64,
    pub decision_type: DecisionType,
    pub reason: String,
    pub confidence: Q16,  // 0.0 - 1.0
    pub factors: Vec<DecisionFactor>,
    pub outcome: Option<Outcome>,
}

impl AiDecision {
    pub fn new(
        id: u64,
        timestamp: u64,
        decision_type: DecisionType,
        reason: &str,
        confidence: Q16,
    ) -> Self {
        AiDecision {
            id,
            timestamp,
            decision_type,
            reason: String::from(reason),
            confidence,
            factors: Vec::new(),
            outcome: None,
        }
    }

    pub fn with_factor(mut self, factor: DecisionFactor) -> Self {
        self.factors.push(factor);
        self
    }

    pub fn with_factors(mut self, factors: Vec<DecisionFactor>) -> Self {
        self.factors = factors;
        self
    }
}

/// The core explainable AI engine
pub struct ExplainableEngine {
    decision_log: Vec<AiDecision>,
    explanation_level: ExplanationLevel,
    user_feedback: BTreeMap<u64, Outcome>,
    total_decisions: u64,
    total_explained: u64,
    acceptance_rate: Q16,
    decision_counter: u64,
}

impl ExplainableEngine {
    const MAX_DECISIONS: usize = 256;

    pub fn new(explanation_level: ExplanationLevel) -> Self {
        ExplainableEngine {
            decision_log: Vec::new(),
            explanation_level,
            user_feedback: BTreeMap::new(),
            total_decisions: 0,
            total_explained: 0,
            acceptance_rate: Q16::from_i32(0),
            decision_counter: 0,
        }
    }

    /// Log a new AI decision to the ring buffer
    pub fn log_decision(&mut self, mut decision: AiDecision) {
        decision.id = self.decision_counter;
        self.decision_counter = self.decision_counter.wrapping_add(1);

        // Maintain ring buffer (max 256 decisions)
        if self.decision_log.len() >= Self::MAX_DECISIONS {
            self.decision_log.remove(0);
        }

        self.decision_log.push(decision);
        self.total_decisions = self.total_decisions.saturating_add(1);
    }

    /// Generate a natural language explanation of a decision
    pub fn generate_explanation(&self, decision_id: u64) -> Option<String> {
        let decision = self.decision_log.iter().find(|d| d.id == decision_id)?;

        let mut explanation = String::new();

        match self.explanation_level {
            ExplanationLevel::Silent => {
                return None;
            }
            ExplanationLevel::Summary => {
                // One-liner with decision type and confidence
                explanation.push_str("AI: ");
                explanation.push_str(decision.decision_type.as_str());
                explanation.push_str(" (");

                let conf_pct = (decision.confidence.to_i32() * 100) / Q16::ONE.to_i32();
                if conf_pct >= 0 && conf_pct <= 100 {
                    let _ = write!(&mut explanation, "{}%", conf_pct);
                }
                explanation.push_str(" confidence). ");
                explanation.push_str(&decision.reason);

                self.total_explained = self.total_explained.saturating_add(1);
            }
            ExplanationLevel::Detailed => {
                // Multi-sentence with top factors
                explanation.push_str("I decided to ");
                explanation.push_str(decision.decision_type.as_str());
                explanation.push_str(" because ");
                explanation.push_str(&decision.reason);
                explanation.push_str(". ");

                // Top 3 factors by weight
                let mut sorted_factors = decision.factors.clone();
                sorted_factors.sort_by(|a, b| {
                    // Simple comparison since we can't use floating-point directly
                    if a.weight.to_i32() > b.weight.to_i32() {
                        core::cmp::Ordering::Less
                    } else if a.weight.to_i32() < b.weight.to_i32() {
                        core::cmp::Ordering::Greater
                    } else {
                        core::cmp::Ordering::Equal
                    }
                });

                if !sorted_factors.is_empty() {
                    explanation.push_str("Key factors: ");
                    for (i, factor) in sorted_factors.iter().take(3).enumerate() {
                        if i > 0 {
                            explanation.push_str(", ");
                        }
                        explanation.push_str(&factor.name);
                        explanation.push_str(" (");
                        explanation.push_str(&factor.description);
                        explanation.push_str(")");
                    }
                    explanation.push_str(". ");
                }

                let conf_pct = (decision.confidence.to_i32() * 100) / Q16::ONE.to_i32();
                if conf_pct >= 0 && conf_pct <= 100 {
                    let _ = write!(&mut explanation, "Confidence: {}%. ", conf_pct);
                }

                self.total_explained = self.total_explained.saturating_add(1);
            }
            ExplanationLevel::Debug => {
                // Full technical details
                explanation.push_str("=== DEBUG EXPLANATION ===\n");
                explanation.push_str("Decision ID: ");
                let _ = write!(&mut explanation, "{}\n", decision.id);
                explanation.push_str("Type: ");
                explanation.push_str(decision.decision_type.as_str());
                explanation.push_str("\n");
                explanation.push_str("Timestamp: ");
                let _ = write!(&mut explanation, "{}\n", decision.timestamp);
                explanation.push_str("Reason: ");
                explanation.push_str(&decision.reason);
                explanation.push_str("\n");

                let conf_pct = (decision.confidence.to_i32() * 100) / Q16::ONE.to_i32();
                explanation.push_str("Confidence: ");
                let _ = write!(&mut explanation, "{}%\n", conf_pct);

                explanation.push_str("Factors: ");
                let _ = write!(&mut explanation, "{}\n", decision.factors.len());

                for (i, factor) in decision.factors.iter().enumerate() {
                    explanation.push_str("  [");
                    let _ = write!(&mut explanation, "{}] {} = ", i, factor.name);
                    let val_pct = (factor.value.to_i32() * 100) / Q16::ONE.to_i32();
                    let _ = write!(&mut explanation, "{}% (weight: ", val_pct);
                    let weight_pct = (factor.weight.to_i32() * 100) / Q16::ONE.to_i32();
                    let _ = write!(&mut explanation, "{}%)\n", weight_pct);
                }

                if let Some(outcome) = decision.outcome {
                    explanation.push_str("Outcome: ");
                    explanation.push_str(match outcome {
                        Outcome::Accepted => "ACCEPTED",
                        Outcome::Rejected => "REJECTED",
                        Outcome::Deferred => "DEFERRED",
                        Outcome::Failed => "FAILED",
                        Outcome::NoFeedback => "NO_FEEDBACK",
                    });
                    explanation.push_str("\n");
                }

                self.total_explained = self.total_explained.saturating_add(1);
            }
        }

        Some(explanation)
    }

    /// Record user feedback on a decision
    pub fn provide_feedback(&mut self, decision_id: u64, outcome: Outcome) {
        self.user_feedback.insert(decision_id, outcome);

        // Update the decision's outcome field
        if let Some(decision) = self
            .decision_log
            .iter_mut()
            .find(|d| d.id == decision_id)
        {
            decision.outcome = Some(outcome);
        }

        // Recalculate acceptance rate
        self.update_acceptance_rate();
    }

    fn update_acceptance_rate(&mut self) {
        if self.user_feedback.is_empty() {
            self.acceptance_rate = Q16::from_i32(0);
            return;
        }

        let accepted = self
            .user_feedback
            .values()
            .filter(|&&o| o == Outcome::Accepted)
            .count() as i32;
        let total = self.user_feedback.len() as i32;

        if total > 0 {
            // acceptance_rate = (accepted / total) as Q16
            self.acceptance_rate = Q16::from_i32(accepted * Q16::ONE.to_i32() / total);
        }
    }

    /// Get the N most recent decisions
    pub fn get_recent_decisions(&self, count: usize) -> Vec<AiDecision> {
        self.decision_log
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect()
    }

    /// Get aggregate statistics
    pub fn get_stats(&self) -> (u64, u64, u64, Q16) {
        (
            self.total_decisions,
            self.total_explained,
            self.user_feedback.len() as u64,
            self.acceptance_rate,
        )
    }

    /// Get most recently rejected decisions
    pub fn get_most_rejected(&self) -> Vec<AiDecision> {
        self.decision_log
            .iter()
            .filter(|d| d.outcome == Some(Outcome::Rejected))
            .cloned()
            .collect()
    }

    /// Get all decisions of a specific type
    pub fn get_decisions_by_type(&self, decision_type: DecisionType) -> Vec<AiDecision> {
        self.decision_log
            .iter()
            .filter(|d| d.decision_type == decision_type)
            .cloned()
            .collect()
    }

    /// Set the explanation verbosity level
    pub fn set_explanation_level(&mut self, level: ExplanationLevel) {
        self.explanation_level = level;
    }

    /// Get current explanation level
    pub fn get_explanation_level(&self) -> ExplanationLevel {
        self.explanation_level
    }

    /// Clear all decision history (but preserve stats)
    pub fn clear_history(&mut self) {
        self.decision_log.clear();
    }
}

use core::fmt::Write;

static mut EXPLAINABLE_ENGINE: Option<Mutex<ExplainableEngine>> = None;

/// Initialize the explainable AI engine with a given verbosity level
pub fn init(explanation_level: ExplanationLevel) {
    unsafe {
        EXPLAINABLE_ENGINE = Some(Mutex::new(ExplainableEngine::new(explanation_level)));
    }
}

/// Log a decision to the engine
pub fn log_decision(decision: AiDecision) {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(mut eng) = engine.lock() {
            eng.log_decision(decision);
        }
    }
}

/// Explain a specific decision
pub fn explain(decision_id: u64) -> Option<String> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return eng.generate_explanation(decision_id);
        }
    }
    None
}

/// Provide user feedback on a decision
pub fn provide_feedback(decision_id: u64, outcome: Outcome) {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(mut eng) = engine.lock() {
            eng.provide_feedback(decision_id, outcome);
        }
    }
}

/// Get the N most recent decisions
pub fn recent_decisions(count: usize) -> Option<Vec<AiDecision>> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return Some(eng.get_recent_decisions(count));
        }
    }
    None
}

/// Get aggregate statistics (total_decisions, total_explained, feedback_count, acceptance_rate)
pub fn stats() -> Option<(u64, u64, u64, Q16)> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return Some(eng.get_stats());
        }
    }
    None
}

/// Get decisions rejected by the user
pub fn most_rejected() -> Option<Vec<AiDecision>> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return Some(eng.get_most_rejected());
        }
    }
    None
}

/// Get decisions of a specific type
pub fn decisions_by_type(decision_type: DecisionType) -> Option<Vec<AiDecision>> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return Some(eng.get_decisions_by_type(decision_type));
        }
    }
    None
}

/// Set the explanation verbosity level
pub fn set_explanation_level(level: ExplanationLevel) {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(mut eng) = engine.lock() {
            eng.set_explanation_level(level);
        }
    }
}

/// Get current explanation level
pub fn get_explanation_level() -> Option<ExplanationLevel> {
    if let Some(engine) = unsafe { EXPLAINABLE_ENGINE.as_ref() } {
        if let Ok(eng) = engine.lock() {
            return Some(eng.get_explanation_level());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decision_creation() {
        let decision = AiDecision::new(
            1,
            1000,
            DecisionType::LayoutChange,
            "User was idle",
            Q16::from_i32(85),
        );
        assert_eq!(decision.id, 1);
        assert_eq!(decision.decision_type, DecisionType::LayoutChange);
        assert!(decision.outcome.is_none());
    }

    #[test]
    fn test_engine_ring_buffer() {
        let mut engine = ExplainableEngine::new(ExplanationLevel::Summary);

        // Add more than MAX_DECISIONS
        for i in 0..300 {
            let decision =
                AiDecision::new(i, 1000 + i, DecisionType::ColorAdaptation, "test", Q16::from_i32(50));
            engine.log_decision(decision);
        }

        // Should only have MAX_DECISIONS
        assert_eq!(engine.decision_log.len(), ExplainableEngine::MAX_DECISIONS);
    }

    #[test]
    fn test_factor_creation() {
        let factor = DecisionFactor::new(
            "battery_level",
            Q16::from_i32(75),
            Q16::from_i32(25),
            "Battery at 25%",
        );
        assert_eq!(factor.name, "battery_level");
    }
}

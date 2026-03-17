use crate::sync::Mutex;
/// Action result evaluation and scoring
///
/// Part of the AIOS agent layer. Evaluates completed agent actions
/// against expectations, scores outcomes, and maintains a history
/// for adaptive planning.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Evaluation verdict for a completed action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Success,
    PartialSuccess,
    Failure,
    Retry,
    Timeout,
    Blocked, // Blocked by safety/permissions
}

/// Confidence level in the verdict
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    High,
    Medium,
    Low,
    Unknown,
}

/// A scored evaluation record
#[derive(Clone, Copy)]
pub struct EvalRecord {
    pub action_hash: u64,
    pub verdict: Verdict,
    pub confidence: Confidence,
    pub score: i32, // -100 to +100
    pub timestamp: u64,
    pub duration_ms: u32,
    pub session_id: u32,
    pub retry_count: u8,
}

/// Evaluation criteria configuration
struct EvalCriteria {
    strict_mode: bool,          // Strict = PartialSuccess counts as Failure
    max_retries: u8,            // Max retries before declaring failure
    timeout_ms: u32,            // Action timeout threshold
    min_confidence: Confidence, // Below this = flag for review
}

struct EvaluatorInner {
    criteria: EvalCriteria,
    history: Vec<EvalRecord>,
    total_evaluated: u64,
    total_success: u64,
    total_failure: u64,
    total_retries: u64,
    // Running success rate (fixed-point, 0-10000 = 0.00-100.00%)
    success_rate: u32,
}

static EVALUATOR: Mutex<Option<EvaluatorInner>> = Mutex::new(None);

impl EvaluatorInner {
    fn new() -> Self {
        EvaluatorInner {
            criteria: EvalCriteria {
                strict_mode: false,
                max_retries: 3,
                timeout_ms: 120_000,
                min_confidence: Confidence::Low,
            },
            history: Vec::new(),
            total_evaluated: 0,
            total_success: 0,
            total_failure: 0,
            total_retries: 0,
            success_rate: 10000, // Start at 100%
        }
    }

    /// Evaluate an action's output against expectations
    fn evaluate(
        &mut self,
        action_hash: u64,
        expected_hash: u64,
        actual_hash: u64,
        duration_ms: u32,
        exit_code: i32,
        session_id: u32,
        timestamp: u64,
    ) -> EvalRecord {
        self.total_evaluated = self.total_evaluated.saturating_add(1);

        // Determine verdict
        let verdict = if duration_ms > self.criteria.timeout_ms {
            Verdict::Timeout
        } else if exit_code == 0 && expected_hash == actual_hash {
            Verdict::Success
        } else if exit_code == 0 {
            // Completed but output differs
            Verdict::PartialSuccess
        } else if exit_code == -1 {
            // Blocked by safety/permissions
            Verdict::Blocked
        } else {
            Verdict::Failure
        };

        // Determine confidence
        let confidence = if expected_hash == 0 {
            Confidence::Unknown // No expected output to compare
        } else if expected_hash == actual_hash {
            Confidence::High
        } else if exit_code == 0 {
            Confidence::Medium
        } else {
            Confidence::Low
        };

        // Calculate score: -100 to +100
        let score = match verdict {
            Verdict::Success => 100,
            Verdict::PartialSuccess => {
                if self.criteria.strict_mode {
                    -20
                } else {
                    60
                }
            }
            Verdict::Retry => 0,
            Verdict::Timeout => -50,
            Verdict::Blocked => -30,
            Verdict::Failure => -100,
        };

        // Update counters
        match verdict {
            Verdict::Success | Verdict::PartialSuccess => {
                self.total_success = self.total_success.saturating_add(1)
            }
            Verdict::Failure | Verdict::Timeout | Verdict::Blocked => {
                self.total_failure = self.total_failure.saturating_add(1)
            }
            Verdict::Retry => self.total_retries = self.total_retries.saturating_add(1),
        }

        // Update rolling success rate (exponential moving average)
        let outcome = if matches!(verdict, Verdict::Success | Verdict::PartialSuccess) {
            10000u32
        } else {
            0u32
        };
        // 90% old, 10% new
        self.success_rate = (self.success_rate * 9 + outcome) / 10;

        let record = EvalRecord {
            action_hash,
            verdict,
            confidence,
            score,
            timestamp,
            duration_ms,
            session_id,
            retry_count: 0,
        };
        self.history.push(record);
        record
    }

    /// Check if an action should be retried based on history
    fn should_retry(&self, action_hash: u64) -> bool {
        let retries = self
            .history
            .iter()
            .filter(|r| r.action_hash == action_hash && r.verdict == Verdict::Retry)
            .count();
        (retries as u8) < self.criteria.max_retries
    }

    /// Get average score for a specific action type
    fn action_avg_score(&self, action_hash: u64) -> i32 {
        let records: Vec<_> = self
            .history
            .iter()
            .filter(|r| r.action_hash == action_hash)
            .collect();
        if records.is_empty() {
            return 0;
        }
        let sum: i32 = records.iter().map(|r| r.score).sum();
        sum / records.len() as i32
    }

    /// Get the current success rate as a percentage (0-100)
    fn get_success_rate(&self) -> u32 {
        self.success_rate / 100
    }

    fn set_strict_mode(&mut self, strict: bool) {
        self.criteria.strict_mode = strict;
    }

    fn explain_verdict(verdict: Verdict) -> &'static str {
        match verdict {
            Verdict::Success => "Action completed successfully with expected output",
            Verdict::PartialSuccess => "Action completed but output differs from expected",
            Verdict::Failure => "Action failed with non-zero exit code",
            Verdict::Retry => "Action should be retried",
            Verdict::Timeout => "Action exceeded time limit",
            Verdict::Blocked => "Action blocked by safety or permissions",
        }
    }
}

// --- Public API ---

/// Evaluate a completed action
pub fn evaluate(
    action_hash: u64,
    expected_hash: u64,
    actual_hash: u64,
    duration_ms: u32,
    exit_code: i32,
    session_id: u32,
    timestamp: u64,
) -> Verdict {
    let mut eval = EVALUATOR.lock();
    match eval.as_mut() {
        Some(e) => {
            e.evaluate(
                action_hash,
                expected_hash,
                actual_hash,
                duration_ms,
                exit_code,
                session_id,
                timestamp,
            )
            .verdict
        }
        None => Verdict::Failure,
    }
}

/// Check if an action should be retried
pub fn should_retry(action_hash: u64) -> bool {
    let eval = EVALUATOR.lock();
    match eval.as_ref() {
        Some(e) => e.should_retry(action_hash),
        None => false,
    }
}

/// Get success rate percentage (0-100)
pub fn success_rate() -> u32 {
    let eval = EVALUATOR.lock();
    match eval.as_ref() {
        Some(e) => e.get_success_rate(),
        None => 0,
    }
}

/// Get explanation for a verdict
pub fn explain(verdict: Verdict) -> &'static str {
    EvaluatorInner::explain_verdict(verdict)
}

/// Set strict evaluation mode
pub fn set_strict(strict: bool) {
    let mut eval = EVALUATOR.lock();
    if let Some(e) = eval.as_mut() {
        e.set_strict_mode(strict);
    }
}

pub fn init() {
    let mut eval = EVALUATOR.lock();
    *eval = Some(EvaluatorInner::new());
    serial_println!("    Evaluator: verdict scoring, retry logic, success rate tracking ready");
}

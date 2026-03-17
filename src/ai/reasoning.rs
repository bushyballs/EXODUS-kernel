use crate::sync::Mutex;
/// Chain-of-thought reasoning engine for Genesis
///
/// Step decomposition, verification, backtracking, and confidence
/// scoring — all on-device with Q16 fixed-point math.
///
/// Also includes:
///   - Simple rule engine: IF-THEN rules with pattern matching
///   - Forward chaining: derive new facts from rules
///   - Backward chaining: prove a goal by finding supporting rules
///   - Basic propositional logic: AND, OR, NOT evaluation
///
/// No data ever leaves the device. All reasoning is local.
///
/// Inspired by: Chain-of-Thought prompting, Tree-of-Thought, AlphaProof. All code is original.
use crate::{serial_print, serial_println};
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Q16 fixed-point constant: 1.0 = 65536
const Q16_ONE: i32 = 65536;

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

/// Q16 from integer
const fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Q16 square root via Newton-Raphson
fn q16_sqrt(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 16 {
        let div = q16_div(x, guess);
        guess = (guess + div) / 2;
        i += 1;
    }
    guess
}

// ---------------------------------------------------------------------------
// Propositional logic
// ---------------------------------------------------------------------------

/// A propositional logic expression
#[derive(Clone)]
pub enum Proposition {
    /// An atomic fact, identified by name
    Atom(String),
    /// Logical NOT
    Not(Vec<Proposition>), // Vec of size 1 (Box requires alloc::boxed which is fine, but Vec works too)
    /// Logical AND of sub-propositions
    And(Vec<Proposition>),
    /// Logical OR of sub-propositions
    Or(Vec<Proposition>),
}

impl Proposition {
    pub fn atom(name: &str) -> Self {
        Proposition::Atom(String::from(name))
    }
    pub fn not(inner: Proposition) -> Self {
        Proposition::Not(vec![inner])
    }
    pub fn and(parts: Vec<Proposition>) -> Self {
        Proposition::And(parts)
    }
    pub fn or(parts: Vec<Proposition>) -> Self {
        Proposition::Or(parts)
    }

    /// Evaluate this proposition against a set of known-true facts
    pub fn evaluate(&self, facts: &[String]) -> bool {
        match self {
            Proposition::Atom(name) => facts.iter().any(|f| f == name),
            Proposition::Not(inner) => {
                if let Some(p) = inner.first() {
                    !p.evaluate(facts)
                } else {
                    true
                }
            }
            Proposition::And(parts) => parts.iter().all(|p| p.evaluate(facts)),
            Proposition::Or(parts) => parts.iter().any(|p| p.evaluate(facts)),
        }
    }

    /// Collect all atom names referenced in this proposition
    pub fn atoms(&self) -> Vec<String> {
        let mut result = Vec::new();
        self.collect_atoms(&mut result);
        result
    }

    fn collect_atoms(&self, out: &mut Vec<String>) {
        match self {
            Proposition::Atom(name) => {
                if !out.iter().any(|a| a == name) {
                    out.push(name.clone());
                }
            }
            Proposition::Not(inner) => {
                for p in inner {
                    p.collect_atoms(out);
                }
            }
            Proposition::And(parts) | Proposition::Or(parts) => {
                for p in parts {
                    p.collect_atoms(out);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule engine
// ---------------------------------------------------------------------------

/// An IF-THEN rule: if all conditions are satisfied, assert the conclusions
pub struct Rule {
    pub id: u32,
    pub name: String,
    /// Conditions that must hold (propositional expressions)
    pub conditions: Proposition,
    /// Facts to assert when the rule fires
    pub conclusions: Vec<String>,
    /// Confidence of the rule (Q16)
    pub confidence: i32,
    /// Number of times this rule has fired
    pub fire_count: u32,
}

/// The rule engine manages facts and rules, and supports forward/backward chaining
pub struct RuleEngine {
    pub rules: Vec<Rule>,
    pub facts: Vec<String>,
    pub next_rule_id: u32,
    pub max_iterations: u32,
}

impl RuleEngine {
    const fn new() -> Self {
        RuleEngine {
            rules: Vec::new(),
            facts: Vec::new(),
            next_rule_id: 1,
            max_iterations: 100,
        }
    }

    /// Assert a fact
    pub fn assert_fact(&mut self, fact: &str) {
        let s = String::from(fact);
        if !self.facts.iter().any(|f| f == &s) {
            self.facts.push(s);
        }
    }

    /// Retract a fact
    pub fn retract_fact(&mut self, fact: &str) {
        self.facts.retain(|f| f != fact);
    }

    /// Check if a fact is known
    pub fn has_fact(&self, fact: &str) -> bool {
        self.facts.iter().any(|f| f == fact)
    }

    /// Add a rule to the engine
    pub fn add_rule(
        &mut self,
        name: &str,
        conditions: Proposition,
        conclusions: Vec<String>,
        confidence: i32,
    ) -> u32 {
        let id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);
        self.rules.push(Rule {
            id,
            name: String::from(name),
            conditions,
            conclusions,
            confidence,
            fire_count: 0,
        });
        id
    }

    /// Forward chaining: repeatedly apply all rules whose conditions are met,
    /// asserting new facts until no more rules fire (fixed-point iteration).
    ///
    /// Returns the number of new facts derived.
    pub fn forward_chain(&mut self) -> u32 {
        let mut total_new = 0u32;

        for _iteration in 0..self.max_iterations {
            let mut new_facts_this_round: Vec<String> = Vec::new();

            for rule in &mut self.rules {
                if rule.conditions.evaluate(&self.facts) {
                    for conclusion in &rule.conclusions {
                        if !self.facts.iter().any(|f| f == conclusion)
                            && !new_facts_this_round.iter().any(|f| f == conclusion)
                        {
                            new_facts_this_round.push(conclusion.clone());
                            rule.fire_count += 1;
                        }
                    }
                }
            }

            if new_facts_this_round.is_empty() {
                break; // Fixed point reached
            }

            total_new += new_facts_this_round.len() as u32;
            for fact in new_facts_this_round {
                self.facts.push(fact);
            }
        }
        total_new
    }

    /// Backward chaining: given a goal fact, determine if it can be proved.
    ///
    /// Returns a proof trace: list of (rule_id, rule_name) that support the goal,
    /// or an empty list if the goal cannot be proved.
    pub fn backward_chain(&self, goal: &str) -> Vec<(u32, String)> {
        let mut visited = Vec::new();
        let mut proof = Vec::new();
        if self.prove_goal(goal, &mut visited, &mut proof, 0) {
            proof
        } else {
            Vec::new()
        }
    }

    /// Recursive backward chaining helper
    fn prove_goal(
        &self,
        goal: &str,
        visited: &mut Vec<String>,
        proof: &mut Vec<(u32, String)>,
        depth: u32,
    ) -> bool {
        if depth > 20 {
            return false;
        } // Depth limit to prevent infinite recursion

        // Base case: the goal is already a known fact
        if self.facts.iter().any(|f| f == goal) {
            return true;
        }

        // Cycle detection
        let goal_str = String::from(goal);
        if visited.iter().any(|v| v == &goal_str) {
            return false;
        }
        visited.push(goal_str);

        // Find rules that conclude the goal
        for rule in &self.rules {
            if !rule.conclusions.iter().any(|c| c == goal) {
                continue;
            }

            // Try to prove all atoms in the rule's conditions
            let required_atoms = rule.conditions.atoms();
            let all_proved = required_atoms
                .iter()
                .all(|atom| self.prove_goal(atom, visited, proof, depth + 1));

            if all_proved && rule.conditions.evaluate(&self.facts) {
                proof.push((rule.id, rule.name.clone()));
                return true;
            }

            // If the conditions include NOT, check if we can still satisfy
            // by evaluating the proposition directly assuming proved atoms are facts
            let mut augmented_facts = self.facts.clone();
            for atom in &required_atoms {
                if self.prove_goal(atom, visited, proof, depth + 1) {
                    let s = String::from(atom.as_str());
                    if !augmented_facts.iter().any(|f| f == &s) {
                        augmented_facts.push(s);
                    }
                }
            }
            if rule.conditions.evaluate(&augmented_facts) {
                proof.push((rule.id, rule.name.clone()));
                return true;
            }
        }

        false
    }

    /// Evaluate a propositional expression against current facts
    pub fn evaluate(&self, prop: &Proposition) -> bool {
        prop.evaluate(&self.facts)
    }

    pub fn fact_count(&self) -> usize {
        self.facts.len()
    }
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

// ---------------------------------------------------------------------------
// Reasoning step types (chain-of-thought)
// ---------------------------------------------------------------------------

/// Maximum reasoning steps in a single chain
const MAX_CHAIN_STEPS: usize = 32;

/// Maximum branches explored in tree-of-thought
const MAX_BRANCHES: usize = 64;

/// Maximum reasoning chains tracked by the engine
const MAX_CHAINS: usize = 128;

/// Maximum verification checks per step
const MAX_CHECKS_PER_STEP: usize = 8;

/// The type of reasoning operation performed at a step
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepType {
    Decompose,   // Break a problem into sub-problems
    Lookup,      // Retrieve known facts or context
    Infer,       // Draw a conclusion from premises
    Calculate,   // Perform a computation
    Compare,     // Compare two values or concepts
    Hypothesize, // Propose a hypothesis
    Verify,      // Check a step against constraints
    Synthesize,  // Combine multiple results
    Backtrack,   // Undo a previous step and try alternative
    Conclude,    // Final answer
}

/// State of a reasoning step
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Pending,
    InProgress,
    Verified,
    Failed,
    Backtracked,
    Skipped,
}

/// A single reasoning step in a chain-of-thought
pub struct ReasoningStep {
    pub step_id: u32,
    pub step_type: StepType,
    pub state: StepState,
    pub premise: String,    // input to this step
    pub conclusion: String, // output of this step
    pub confidence: i32,    // Q16 confidence in this step
    pub parent_step: Option<u32>,
    pub verification_checks: Vec<VerificationCheck>,
    pub alternatives_tried: u32,
    pub depth: u32,
}

/// A verification check applied to a reasoning step
pub struct VerificationCheck {
    pub check_type: CheckType,
    pub passed: bool,
    pub detail: String,
    pub severity: i32, // Q16 how much this affects confidence
}

/// Types of verification checks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckType {
    ConsistencyCheck,  // Does this step contradict earlier ones?
    FactCheck,         // Is the premise factually supported?
    LogicCheck,        // Is the inference logically valid?
    BoundsCheck,       // Are computed values within expected ranges?
    RelevanceCheck,    // Is this step relevant to the original query?
    CompletenessCheck, // Are there missing considerations?
    CircularityCheck,  // Does this create circular reasoning?
    PremiseCheck,      // Are all premises established?
}

// ---------------------------------------------------------------------------
// Reasoning chain
// ---------------------------------------------------------------------------

/// State of the overall reasoning chain
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainState {
    Building,
    Verifying,
    Complete,
    Failed,
    Backtracking,
}

/// A complete chain-of-thought reasoning trace
pub struct ReasoningChain {
    pub id: u32,
    pub query: String,
    pub steps: Vec<ReasoningStep>,
    pub state: ChainState,
    pub overall_confidence: i32, // Q16
    pub next_step_id: u32,
    pub backtrack_count: u32,
    pub max_depth: u32,
    pub created_at: u64,
    pub completed_at: u64,
}

impl ReasoningChain {
    /// Create a new reasoning chain for a query
    fn new(id: u32, query: &str) -> Self {
        let now = crate::time::clock::unix_time();
        ReasoningChain {
            id,
            query: String::from(query),
            steps: Vec::new(),
            state: ChainState::Building,
            overall_confidence: Q16_ONE,
            next_step_id: 1,
            backtrack_count: 0,
            max_depth: 10,
            created_at: now,
            completed_at: 0,
        }
    }

    /// Add a reasoning step to the chain
    pub fn add_step(
        &mut self,
        step_type: StepType,
        premise: &str,
        conclusion: &str,
        confidence: i32,
        parent: Option<u32>,
    ) -> u32 {
        if self.steps.len() >= MAX_CHAIN_STEPS {
            return 0;
        }
        let step_id = self.next_step_id;
        self.next_step_id = self.next_step_id.saturating_add(1);

        let depth = if let Some(pid) = parent {
            self.steps
                .iter()
                .find(|s| s.step_id == pid)
                .map(|s| s.depth + 1)
                .unwrap_or(0)
        } else {
            0
        };

        self.steps.push(ReasoningStep {
            step_id,
            step_type,
            state: StepState::InProgress,
            premise: String::from(premise),
            conclusion: String::from(conclusion),
            confidence,
            parent_step: parent,
            verification_checks: Vec::new(),
            alternatives_tried: 0,
            depth,
        });
        step_id
    }

    /// Apply a verification check to a step
    pub fn verify_step(
        &mut self,
        step_id: u32,
        check_type: CheckType,
        passed: bool,
        detail: &str,
        severity: i32,
    ) {
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_id == step_id) {
            if step.verification_checks.len() >= MAX_CHECKS_PER_STEP {
                return;
            }
            step.verification_checks.push(VerificationCheck {
                check_type,
                passed,
                detail: String::from(detail),
                severity,
            });

            if !passed {
                let penalty = q16_mul(severity, Q16_ONE / 4);
                step.confidence = step.confidence.saturating_sub(penalty);
                if step.confidence < 0 {
                    step.confidence = 0;
                }
            }

            let all_passed = step.verification_checks.iter().all(|c| c.passed);
            let any_critical_fail = step
                .verification_checks
                .iter()
                .any(|c| !c.passed && c.severity > Q16_ONE / 2);
            if any_critical_fail {
                step.state = StepState::Failed;
            } else if all_passed {
                step.state = StepState::Verified;
            }
        }
    }

    /// Backtrack from a failed step: mark it and its dependents
    pub fn backtrack(&mut self, step_id: u32) {
        self.backtrack_count = self.backtrack_count.saturating_add(1);
        self.state = ChainState::Backtracking;

        if let Some(step) = self.steps.iter_mut().find(|s| s.step_id == step_id) {
            step.state = StepState::Backtracked;
            step.alternatives_tried += 1;
        }

        let mut to_backtrack: Vec<u32> = vec![step_id];
        let mut idx = 0;
        while idx < to_backtrack.len() {
            let parent_id = to_backtrack[idx];
            let children: Vec<u32> = self
                .steps
                .iter()
                .filter(|s| s.parent_step == Some(parent_id) && s.state != StepState::Backtracked)
                .map(|s| s.step_id)
                .collect();
            to_backtrack.extend(children);
            idx += 1;
        }
        for sid in &to_backtrack {
            if let Some(step) = self.steps.iter_mut().find(|s| s.step_id == *sid) {
                step.state = StepState::Backtracked;
            }
        }
    }

    /// Compute the overall confidence by aggregating active step confidences
    pub fn compute_confidence(&mut self) -> i32 {
        let active: Vec<i32> = self
            .steps
            .iter()
            .filter(|s| s.state == StepState::Verified || s.state == StepState::InProgress)
            .map(|s| s.confidence)
            .collect();
        if active.is_empty() {
            self.overall_confidence = 0;
            return 0;
        }

        let mut product: i32 = Q16_ONE;
        for c in &active {
            product = q16_mul(product, *c);
        }

        let backtrack_penalty = q16_mul(q16_from_int(self.backtrack_count as i32), Q16_ONE / 20);
        product = product.saturating_sub(backtrack_penalty);
        if product < 0 {
            product = 0;
        }

        self.overall_confidence = product;
        product
    }

    /// Mark the chain as complete with a final conclusion
    pub fn conclude(&mut self, conclusion: &str, confidence: i32) -> u32 {
        let step_id = self.add_step(
            StepType::Conclude,
            "Synthesized from verified reasoning steps",
            conclusion,
            confidence,
            None,
        );
        if let Some(step) = self.steps.iter_mut().find(|s| s.step_id == step_id) {
            step.state = StepState::Verified;
        }
        self.compute_confidence();
        self.state = ChainState::Complete;
        self.completed_at = crate::time::clock::unix_time();
        step_id
    }

    /// Get a flat trace of the reasoning (active steps only)
    pub fn trace(&self) -> Vec<(u32, StepType, StepState, i32)> {
        self.steps
            .iter()
            .filter(|s| s.state != StepState::Backtracked && s.state != StepState::Skipped)
            .map(|s| (s.step_id, s.step_type, s.state, s.confidence))
            .collect()
    }

    /// Count of active (non-backtracked) steps
    pub fn active_step_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| s.state != StepState::Backtracked && s.state != StepState::Skipped)
            .count()
    }

    /// Check if any step has consistency issues with another
    pub fn check_consistency(&self) -> Vec<(u32, u32, String)> {
        let mut issues = Vec::new();
        let active: Vec<&ReasoningStep> = self
            .steps
            .iter()
            .filter(|s| s.state == StepState::Verified || s.state == StepState::InProgress)
            .collect();

        for i in 0..active.len() {
            for j in (i + 1)..active.len() {
                if active[i].premise == active[j].premise
                    && active[i].conclusion != active[j].conclusion
                    && !active[i].premise.is_empty()
                {
                    issues.push((
                        active[i].step_id,
                        active[j].step_id,
                        format!(
                            "Contradictory conclusions for premise: {}",
                            &active[i].premise
                        ),
                    ));
                }
            }
        }
        issues
    }
}

// ---------------------------------------------------------------------------
// Reasoning engine
// ---------------------------------------------------------------------------

/// The chain-of-thought reasoning engine, which also owns the rule engine
pub struct ReasoningEngine {
    pub chains: Vec<ReasoningChain>,
    pub next_chain_id: u32,
    pub total_steps_processed: u64,
    pub total_backtracks: u64,
    pub auto_verify: bool,
    pub min_step_confidence: i32, // Q16 — steps below this trigger backtrack
    pub max_chain_depth: u32,
    /// Embedded rule engine for logical reasoning
    pub rule_engine: RuleEngine,
}

impl ReasoningEngine {
    const fn new() -> Self {
        ReasoningEngine {
            chains: Vec::new(),
            next_chain_id: 1,
            total_steps_processed: 0,
            total_backtracks: 0,
            auto_verify: true,
            min_step_confidence: Q16_ONE / 4, // 0.25
            max_chain_depth: 10,
            rule_engine: RuleEngine::new(),
        }
    }

    /// Begin a new reasoning chain for a query
    pub fn begin_chain(&mut self, query: &str) -> u32 {
        let id = self.next_chain_id;
        self.next_chain_id = self.next_chain_id.saturating_add(1);
        let chain = ReasoningChain::new(id, query);
        self.chains.push(chain);
        if self.chains.len() > MAX_CHAINS {
            if let Some(pos) = self
                .chains
                .iter()
                .position(|c| c.state == ChainState::Complete)
            {
                self.chains.remove(pos);
            }
        }
        id
    }

    /// Add a step to an existing chain
    pub fn add_step(
        &mut self,
        chain_id: u32,
        step_type: StepType,
        premise: &str,
        conclusion: &str,
        confidence: i32,
        parent: Option<u32>,
    ) -> u32 {
        let chain = match self.chains.iter_mut().find(|c| c.id == chain_id) {
            Some(c) => c,
            None => return 0,
        };
        let step_id = chain.add_step(step_type, premise, conclusion, confidence, parent);
        self.total_steps_processed = self.total_steps_processed.saturating_add(1);

        if self.auto_verify && step_id > 0 {
            self.auto_verify_step(chain_id, step_id);
        }
        step_id
    }

    /// Run automatic verification checks on a step
    fn auto_verify_step(&mut self, chain_id: u32, step_id: u32) {
        let chain = match self.chains.iter_mut().find(|c| c.id == chain_id) {
            Some(c) => c,
            None => return,
        };

        let (conclusion_empty, premise_empty) = {
            let step = match chain.steps.iter().find(|s| s.step_id == step_id) {
                Some(s) => s,
                None => return,
            };
            (step.conclusion.is_empty(), step.premise.is_empty())
        };

        chain.verify_step(
            step_id,
            CheckType::RelevanceCheck,
            !conclusion_empty,
            if conclusion_empty {
                "Empty conclusion"
            } else {
                "Has conclusion"
            },
            Q16_ONE / 3,
        );

        chain.verify_step(
            step_id,
            CheckType::PremiseCheck,
            !premise_empty,
            if premise_empty {
                "Missing premise"
            } else {
                "Premise present"
            },
            Q16_ONE / 4,
        );

        let issues = chain.check_consistency();
        let has_conflict = issues
            .iter()
            .any(|(a, b, _)| *a == step_id || *b == step_id);
        chain.verify_step(
            step_id,
            CheckType::ConsistencyCheck,
            !has_conflict,
            if has_conflict {
                "Contradicts another step"
            } else {
                "Consistent"
            },
            Q16_ONE / 2,
        );

        // Check if the rule engine can verify the premise as a known fact
        if !premise_empty {
            let premise_str = chain
                .steps
                .iter()
                .find(|s| s.step_id == step_id)
                .map(|s| s.premise.clone())
                .unwrap_or_default();
            let fact_supported = self.rule_engine.has_fact(&premise_str);
            chain.verify_step(
                step_id,
                CheckType::FactCheck,
                fact_supported,
                if fact_supported {
                    "Premise is a known fact"
                } else {
                    "Premise not in fact base"
                },
                Q16_ONE / 6, // Low severity — many valid premises won't be in the DB
            );
        }

        let step_confidence = chain
            .steps
            .iter()
            .find(|s| s.step_id == step_id)
            .map(|s| s.confidence)
            .unwrap_or(0);

        if step_confidence < self.min_step_confidence {
            chain.backtrack(step_id);
            self.total_backtracks = self.total_backtracks.saturating_add(1);
        }
    }

    /// Conclude a reasoning chain
    pub fn conclude_chain(&mut self, chain_id: u32, conclusion: &str, confidence: i32) -> i32 {
        if let Some(chain) = self.chains.iter_mut().find(|c| c.id == chain_id) {
            chain.conclude(conclusion, confidence);
            return chain.overall_confidence;
        }
        0
    }

    /// Get a chain's trace
    pub fn get_trace(&self, chain_id: u32) -> Vec<(u32, StepType, StepState, i32)> {
        self.chains
            .iter()
            .find(|c| c.id == chain_id)
            .map(|c| c.trace())
            .unwrap_or_default()
    }

    /// Get a chain's overall confidence
    pub fn chain_confidence(&self, chain_id: u32) -> i32 {
        self.chains
            .iter()
            .find(|c| c.id == chain_id)
            .map(|c| c.overall_confidence)
            .unwrap_or(0)
    }

    /// Get statistics
    pub fn stats(&self) -> (usize, u64, u64) {
        (
            self.chains.len(),
            self.total_steps_processed,
            self.total_backtracks,
        )
    }

    /// Decompose a query into sub-questions (heuristic)
    pub fn decompose_query(&mut self, chain_id: u32, query: &str) -> Vec<u32> {
        let chain = match self.chains.iter_mut().find(|c| c.id == chain_id) {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut step_ids = Vec::new();

        let parts: Vec<&str> = query
            .split(|c: char| c == '?' || c == ',' || c == ';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if parts.len() <= 1 {
            let sid = chain.add_step(
                StepType::Decompose,
                query,
                "Single atomic question — no further decomposition needed",
                Q16_ONE * 9 / 10,
                None,
            );
            step_ids.push(sid);
        } else {
            let root = chain.add_step(
                StepType::Decompose,
                query,
                &format!("Decomposed into {} sub-questions", parts.len()),
                Q16_ONE * 8 / 10,
                None,
            );
            step_ids.push(root);

            for part in &parts {
                let sid = chain.add_step(
                    StepType::Decompose,
                    part,
                    "Sub-question identified",
                    Q16_ONE * 7 / 10,
                    Some(root),
                );
                step_ids.push(sid);
                self.total_steps_processed = self.total_steps_processed.saturating_add(1);
            }
        }
        self.total_steps_processed = self.total_steps_processed.saturating_add(1);
        step_ids
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static REASONING: Mutex<Option<ReasoningEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = ReasoningEngine::new();

    // Seed the rule engine with some basic system rules
    engine.rule_engine.assert_fact("system_running");
    engine.rule_engine.assert_fact("local_mode");

    // Rule: if system is running and local mode, then AI is available
    engine.rule_engine.add_rule(
        "ai_available",
        Proposition::and(vec![
            Proposition::atom("system_running"),
            Proposition::atom("local_mode"),
        ]),
        vec![String::from("ai_available")],
        Q16_ONE,
    );

    // Rule: if AI is available and memory is loaded, then context-aware
    engine.rule_engine.add_rule(
        "context_aware",
        Proposition::and(vec![
            Proposition::atom("ai_available"),
            Proposition::atom("memory_loaded"),
        ]),
        vec![String::from("context_aware")],
        Q16_ONE * 9 / 10,
    );

    // Rule: if AI is available and not offline_mode, then can_search
    engine.rule_engine.add_rule(
        "can_search",
        Proposition::and(vec![
            Proposition::atom("ai_available"),
            Proposition::not(Proposition::atom("offline_mode")),
        ]),
        vec![String::from("can_search")],
        Q16_ONE * 8 / 10,
    );

    // Run forward chaining to derive initial facts
    let derived = engine.rule_engine.forward_chain();

    *REASONING.lock() = Some(engine);
    serial_println!(
        "    [reasoning] Chain-of-thought + rule engine initialized ({} facts derived)",
        derived
    );
}

/// Begin a new reasoning chain
pub fn begin_chain(query: &str) -> u32 {
    REASONING
        .lock()
        .as_mut()
        .map(|e| e.begin_chain(query))
        .unwrap_or(0)
}

/// Add a step to a chain
pub fn add_step(
    chain_id: u32,
    step_type: StepType,
    premise: &str,
    conclusion: &str,
    confidence: i32,
    parent: Option<u32>,
) -> u32 {
    REASONING
        .lock()
        .as_mut()
        .map(|e| e.add_step(chain_id, step_type, premise, conclusion, confidence, parent))
        .unwrap_or(0)
}

/// Decompose a query into sub-questions
pub fn decompose(chain_id: u32, query: &str) -> Vec<u32> {
    REASONING
        .lock()
        .as_mut()
        .map(|e| e.decompose_query(chain_id, query))
        .unwrap_or_default()
}

/// Conclude a reasoning chain
pub fn conclude(chain_id: u32, conclusion: &str, confidence: i32) -> i32 {
    REASONING
        .lock()
        .as_mut()
        .map(|e| e.conclude_chain(chain_id, conclusion, confidence))
        .unwrap_or(0)
}

/// Get a chain's trace
pub fn trace(chain_id: u32) -> Vec<(u32, StepType, StepState, i32)> {
    REASONING
        .lock()
        .as_ref()
        .map(|e| e.get_trace(chain_id))
        .unwrap_or_default()
}

/// Get statistics: (chain_count, total_steps, total_backtracks)
pub fn stats() -> (usize, u64, u64) {
    REASONING
        .lock()
        .as_ref()
        .map(|e| e.stats())
        .unwrap_or((0, 0, 0))
}

/// Assert a fact into the rule engine
pub fn assert_fact(fact: &str) {
    if let Some(engine) = REASONING.lock().as_mut() {
        engine.rule_engine.assert_fact(fact);
    }
}

/// Run forward chaining on the rule engine, returns number of new facts
pub fn forward_chain() -> u32 {
    REASONING
        .lock()
        .as_mut()
        .map(|e| e.rule_engine.forward_chain())
        .unwrap_or(0)
}

/// Backward chain to prove a goal, returns proof trace
pub fn backward_chain(goal: &str) -> Vec<(u32, String)> {
    REASONING
        .lock()
        .as_ref()
        .map(|e| e.rule_engine.backward_chain(goal))
        .unwrap_or_default()
}

/// Evaluate a proposition against current facts
pub fn evaluate_proposition(prop: &Proposition) -> bool {
    REASONING
        .lock()
        .as_ref()
        .map(|e| e.rule_engine.evaluate(prop))
        .unwrap_or(false)
}

/// Add a rule to the rule engine
pub fn add_rule(
    name: &str,
    conditions: Proposition,
    conclusions: Vec<String>,
    confidence: i32,
) -> u32 {
    REASONING
        .lock()
        .as_mut()
        .map(|e| {
            e.rule_engine
                .add_rule(name, conditions, conclusions, confidence)
        })
        .unwrap_or(0)
}

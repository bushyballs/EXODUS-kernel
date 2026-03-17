use crate::sync::Mutex;
use alloc::vec;
/// Self-Improvement Engine — the AI evolves its own prompting
///
/// Tracks which prompt strategies produce the best results and
/// evolves the system prompt, conversation approach, and reasoning
/// patterns over time. The Hoags AI learns from every interaction
/// and refines how it communicates.
///
/// Features:
///   - Prompt strategy tracking with effectiveness scoring
///   - A/B experimentation between strategies
///   - Self-reflection on what worked and what failed
///   - Evolutionary mutation of strategies across generations
///   - Reasoning pattern selection per topic
///   - Continuous improvement rate measurement
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A prompt strategy — a particular template + section ordering + reasoning style
#[derive(Clone)]
pub struct PromptStrategy {
    pub id: u32,
    pub template_hash: u64,
    pub sections: Vec<u64>, // Ordered section hashes
    pub effectiveness: Q16, // Running effectiveness score
    pub uses: u32,
    pub avg_feedback: Q16,
    pub last_used: u64,
}

/// How the AI structures its reasoning
#[derive(Clone, Copy, PartialEq)]
pub enum ReasoningPattern {
    DirectAnswer,
    StepByStep,
    AnalyzeThenAnswer,
    AskClarifyFirst,
    ShowExampleFirst,
    CompareOptions,
    ExplainWhyNot,
}

/// An A/B experiment comparing two strategies
#[derive(Clone, Copy)]
pub struct PromptExperiment {
    pub id: u32,
    pub strategy_a: u32,
    pub strategy_b: u32,
    pub a_wins: u32,
    pub b_wins: u32,
    pub total_trials: u32,
    pub confidence: Q16,
    pub concluded: bool,
}

/// A single self-reflection record
#[derive(Clone, Copy)]
pub struct SelfReflection {
    pub interaction_hash: u64,
    pub what_worked_hash: u64,
    pub what_failed_hash: u64,
    pub improvement_hash: u64,
    pub timestamp: u64,
}

/// How a strategy was mutated to produce a child
#[derive(Clone, Copy, PartialEq)]
pub enum MutationType {
    AddSection,
    RemoveSection,
    ReorderSections,
    ChangeReasoning,
    AdjustVerbosity,
    ChangeExamples,
    MergeStrategies,
}

/// A single evolution step in the strategy lineage
#[derive(Clone, Copy)]
pub struct PromptEvolution {
    pub generation: u32,
    pub parent_strategy: u32,
    pub mutation_type: MutationType,
    pub child_strategy: u32,
    pub fitness_delta: Q16,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct SelfImproveEngine {
    strategies: Vec<PromptStrategy>,
    experiments: Vec<PromptExperiment>,
    reflections: Vec<SelfReflection>,
    evolutions: Vec<PromptEvolution>,
    active_strategy: u32,
    best_strategy: u32,
    generation: u32,
    next_strategy_id: u32,
    next_experiment_id: u32,
    total_improvements: u32,
    current_reasoning: ReasoningPattern,
    improvement_rate: Q16,
}

static ENGINE: Mutex<Option<SelfImproveEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Seed hashes — deterministic constants for the five default strategies.
// All hex digits are valid (0-9, A-F only).
// ---------------------------------------------------------------------------

const SEED_TEMPLATE_1: u64 = 0x00AA_BBCC_DDEE_FF01;
const SEED_TEMPLATE_2: u64 = 0x00AA_BBCC_DDEE_FF02;
const SEED_TEMPLATE_3: u64 = 0x00AA_BBCC_DDEE_FF03;
const SEED_TEMPLATE_4: u64 = 0x00AA_BBCC_DDEE_FF04;
const SEED_TEMPLATE_5: u64 = 0x00AA_BBCC_DDEE_FF05;

const SECTION_IDENTITY: u64 = 0x0000_0000_0000_0001;
const SECTION_RULES: u64 = 0x0000_0000_0000_0002;
const SECTION_CONTEXT: u64 = 0x0000_0000_0000_0003;
const SECTION_EXAMPLES: u64 = 0x0000_0000_0000_0004;
const SECTION_TOOLS: u64 = 0x0000_0000_0000_0005;
const SECTION_MEMORY: u64 = 0x0000_0000_0000_0006;
const SECTION_CAPABILITIES: u64 = 0x0000_0000_0000_0007;

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl SelfImproveEngine {
    /// Create a new engine seeded with five default strategies of varying
    /// verbosity, section order, and reasoning pattern.
    fn new() -> Self {
        let s1 = PromptStrategy {
            id: 1,
            template_hash: SEED_TEMPLATE_1,
            sections: vec![SECTION_IDENTITY, SECTION_RULES, SECTION_CONTEXT],
            effectiveness: q16_from_int(50), // baseline
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        };

        let s2 = PromptStrategy {
            id: 2,
            template_hash: SEED_TEMPLATE_2,
            sections: vec![
                SECTION_IDENTITY,
                SECTION_EXAMPLES,
                SECTION_RULES,
                SECTION_CONTEXT,
            ],
            effectiveness: q16_from_int(50),
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        };

        let s3 = PromptStrategy {
            id: 3,
            template_hash: SEED_TEMPLATE_3,
            sections: vec![
                SECTION_IDENTITY,
                SECTION_CAPABILITIES,
                SECTION_TOOLS,
                SECTION_RULES,
                SECTION_MEMORY,
                SECTION_CONTEXT,
            ],
            effectiveness: q16_from_int(50),
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        };

        let s4 = PromptStrategy {
            id: 4,
            template_hash: SEED_TEMPLATE_4,
            sections: vec![
                SECTION_IDENTITY,
                SECTION_CONTEXT,
                SECTION_EXAMPLES,
                SECTION_RULES,
                SECTION_TOOLS,
            ],
            effectiveness: q16_from_int(50),
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        };

        let s5 = PromptStrategy {
            id: 5,
            template_hash: SEED_TEMPLATE_5,
            sections: vec![SECTION_IDENTITY, SECTION_MEMORY, SECTION_CONTEXT],
            effectiveness: q16_from_int(50),
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        };

        SelfImproveEngine {
            strategies: vec![s1, s2, s3, s4, s5],
            experiments: Vec::new(),
            reflections: Vec::new(),
            evolutions: Vec::new(),
            active_strategy: 1,
            best_strategy: 1,
            generation: 0,
            next_strategy_id: 6,
            next_experiment_id: 1,
            total_improvements: 0,
            current_reasoning: ReasoningPattern::AnalyzeThenAnswer,
            improvement_rate: 0,
        }
    }

    // ------------------------------------------------------------------
    // Strategy management
    // ------------------------------------------------------------------

    /// Register a brand-new strategy and return its id.
    fn create_strategy(&mut self, template: u64, sections: Vec<u64>) -> u32 {
        let id = self.next_strategy_id;
        self.next_strategy_id = self.next_strategy_id.saturating_add(1);
        self.strategies.push(PromptStrategy {
            id,
            template_hash: template,
            sections,
            effectiveness: q16_from_int(50),
            uses: 0,
            avg_feedback: 0,
            last_used: 0,
        });
        id
    }

    /// Return a reference to the currently-active strategy.
    fn get_active_strategy(&self) -> Option<&PromptStrategy> {
        self.strategies
            .iter()
            .find(|s| s.id == self.active_strategy)
    }

    /// Scan all strategies and pick the one with the highest effectiveness.
    fn select_best_strategy(&mut self) -> u32 {
        let mut best_id = self.active_strategy;
        let mut best_eff = 0i32;

        for s in &self.strategies {
            if s.effectiveness > best_eff {
                best_eff = s.effectiveness;
                best_id = s.id;
            }
        }

        self.best_strategy = best_id;
        self.active_strategy = best_id;
        best_id
    }

    // ------------------------------------------------------------------
    // A/B experiments
    // ------------------------------------------------------------------

    /// Begin an A/B experiment between two strategy ids.
    fn start_experiment(&mut self, a: u32, b: u32) -> u32 {
        let id = self.next_experiment_id;
        self.next_experiment_id = self.next_experiment_id.saturating_add(1);
        self.experiments.push(PromptExperiment {
            id,
            strategy_a: a,
            strategy_b: b,
            a_wins: 0,
            b_wins: 0,
            total_trials: 0,
            confidence: 0,
            concluded: false,
        });
        id
    }

    /// Record one trial result for an experiment.
    fn record_experiment_result(&mut self, exp_id: u32, a_won: bool) {
        if let Some(exp) = self
            .experiments
            .iter_mut()
            .find(|e| e.id == exp_id && !e.concluded)
        {
            exp.total_trials += 1;
            if a_won {
                exp.a_wins += 1;
            } else {
                exp.b_wins += 1;
            }

            // Confidence: |a_wins - b_wins| / total * 100  (Q16)
            let diff = if exp.a_wins > exp.b_wins {
                exp.a_wins - exp.b_wins
            } else {
                exp.b_wins - exp.a_wins
            };
            if exp.total_trials > 0 {
                exp.confidence = q16_from_int(diff as i32 * 100) / exp.total_trials as i32;
            }
        }
    }

    /// Try to conclude an experiment. Returns the winner strategy id
    /// if confidence is high enough (>= 70 on a 0-100 scale, Q16).
    fn conclude_experiment(&mut self, exp_id: u32) -> Option<u32> {
        let threshold = q16_from_int(70);
        let min_trials: u32 = 10;

        let winner = {
            let exp = self.experiments.iter().find(|e| e.id == exp_id)?;
            if exp.concluded {
                return None;
            }
            if exp.total_trials < min_trials {
                return None;
            }
            if exp.confidence < threshold {
                return None;
            }

            if exp.a_wins >= exp.b_wins {
                exp.strategy_a
            } else {
                exp.strategy_b
            }
        };

        // Mark concluded
        if let Some(exp) = self.experiments.iter_mut().find(|e| e.id == exp_id) {
            exp.concluded = true;
        }

        // Promote winner
        self.active_strategy = winner;
        self.best_strategy = winner;
        self.total_improvements = self.total_improvements.saturating_add(1);

        Some(winner)
    }

    // ------------------------------------------------------------------
    // Reflection
    // ------------------------------------------------------------------

    /// Store a self-reflection about a past interaction.
    fn reflect(&mut self, interaction: u64, worked: u64, failed: u64, improvement: u64, time: u64) {
        self.reflections.push(SelfReflection {
            interaction_hash: interaction,
            what_worked_hash: worked,
            what_failed_hash: failed,
            improvement_hash: improvement,
            timestamp: time,
        });

        // Keep a bounded history — drop oldest once we exceed 512 entries.
        if self.reflections.len() > 512 {
            self.reflections.remove(0);
        }
    }

    // ------------------------------------------------------------------
    // Evolution
    // ------------------------------------------------------------------

    /// Evolve: create a new strategy by mutating the current best one.
    /// Cycles through mutation types based on generation count.
    fn evolve(&mut self) {
        let mutation = match self.generation % 7 {
            0 => MutationType::AddSection,
            1 => MutationType::RemoveSection,
            2 => MutationType::ReorderSections,
            3 => MutationType::ChangeReasoning,
            4 => MutationType::AdjustVerbosity,
            5 => MutationType::ChangeExamples,
            _ => MutationType::MergeStrategies,
        };

        let parent = self.best_strategy;
        let child_id = self.mutate_strategy(parent, mutation);

        // Record the evolution step (fitness_delta starts at 0, updated later)
        self.evolutions.push(PromptEvolution {
            generation: self.generation,
            parent_strategy: parent,
            mutation_type: mutation,
            child_strategy: child_id,
            fitness_delta: 0,
        });

        self.generation = self.generation.saturating_add(1);

        // Start an experiment: child vs parent
        let _exp = self.start_experiment(parent, child_id);
    }

    /// Create a mutated copy of a parent strategy.
    fn mutate_strategy(&mut self, parent_id: u32, mutation: MutationType) -> u32 {
        // Find parent sections (clone them)
        let parent = self.strategies.iter().find(|s| s.id == parent_id);
        let mut sections = match parent {
            Some(p) => p.sections.clone(),
            None => vec![SECTION_IDENTITY, SECTION_CONTEXT],
        };
        let parent_hash = parent.map_or(0u64, |p| p.template_hash);

        match mutation {
            MutationType::AddSection => {
                // Add a section the parent does not already have
                let candidates = [
                    SECTION_IDENTITY,
                    SECTION_RULES,
                    SECTION_CONTEXT,
                    SECTION_EXAMPLES,
                    SECTION_TOOLS,
                    SECTION_MEMORY,
                    SECTION_CAPABILITIES,
                ];
                for &c in &candidates {
                    if !sections.contains(&c) {
                        sections.push(c);
                        break;
                    }
                }
            }
            MutationType::RemoveSection => {
                // Never remove identity; remove last non-identity section
                if sections.len() > 1 {
                    // Find last section that is not SECTION_IDENTITY
                    let mut idx = sections.len() - 1;
                    while idx > 0 && sections[idx] == SECTION_IDENTITY {
                        idx -= 1;
                    }
                    if sections[idx] != SECTION_IDENTITY {
                        sections.remove(idx);
                    }
                }
            }
            MutationType::ReorderSections => {
                // Rotate sections one position to the right
                if sections.len() > 1 {
                    let last = sections.pop().unwrap_or(SECTION_IDENTITY);
                    sections.insert(0, last);
                }
            }
            MutationType::ChangeReasoning => {
                // Cycle the global reasoning pattern
                self.current_reasoning = match self.current_reasoning {
                    ReasoningPattern::DirectAnswer => ReasoningPattern::StepByStep,
                    ReasoningPattern::StepByStep => ReasoningPattern::AnalyzeThenAnswer,
                    ReasoningPattern::AnalyzeThenAnswer => ReasoningPattern::AskClarifyFirst,
                    ReasoningPattern::AskClarifyFirst => ReasoningPattern::ShowExampleFirst,
                    ReasoningPattern::ShowExampleFirst => ReasoningPattern::CompareOptions,
                    ReasoningPattern::CompareOptions => ReasoningPattern::ExplainWhyNot,
                    ReasoningPattern::ExplainWhyNot => ReasoningPattern::DirectAnswer,
                };
            }
            MutationType::AdjustVerbosity => {
                // Toggle: if examples are present remove them, else add them
                if sections.contains(&SECTION_EXAMPLES) {
                    sections.retain(|&s| s != SECTION_EXAMPLES);
                } else {
                    sections.push(SECTION_EXAMPLES);
                }
            }
            MutationType::ChangeExamples => {
                // Swap examples to front if present, else add at front
                sections.retain(|&s| s != SECTION_EXAMPLES);
                sections.insert(0, SECTION_EXAMPLES);
            }
            MutationType::MergeStrategies => {
                // Merge: union of current best and the second-best strategy
                let mut second_best_eff = 0i32;
                let mut second_best_sections: Option<Vec<u64>> = None;
                for s in &self.strategies {
                    if s.id != self.best_strategy && s.effectiveness > second_best_eff {
                        second_best_eff = s.effectiveness;
                        second_best_sections = Some(s.sections.clone());
                    }
                }
                if let Some(other) = second_best_sections {
                    for sec in other {
                        if !sections.contains(&sec) {
                            sections.push(sec);
                        }
                    }
                }
            }
        }

        // Compute a child template hash by mixing parent hash with mutation ordinal
        let mutation_ord = mutation as u64;
        let child_hash = parent_hash ^ (mutation_ord.wrapping_mul(0x00FF_00FF_00FF_00FF));

        self.create_strategy(child_hash, sections)
    }

    // ------------------------------------------------------------------
    // Reasoning selection
    // ------------------------------------------------------------------

    /// Pick the best reasoning style for a given topic.
    /// Uses a simple hash-based heuristic mapped to patterns.
    fn select_reasoning(&self, topic_hash: u64) -> ReasoningPattern {
        // Distribute topics across patterns via modular arithmetic.
        // Over time, feedback will override this default.
        match (topic_hash % 7) as u8 {
            0 => ReasoningPattern::DirectAnswer,
            1 => ReasoningPattern::StepByStep,
            2 => ReasoningPattern::AnalyzeThenAnswer,
            3 => ReasoningPattern::AskClarifyFirst,
            4 => ReasoningPattern::ShowExampleFirst,
            5 => ReasoningPattern::CompareOptions,
            _ => ReasoningPattern::ExplainWhyNot,
        }
    }

    // ------------------------------------------------------------------
    // Effectiveness tracking
    // ------------------------------------------------------------------

    /// Update a strategy's effectiveness with new feedback.
    /// Uses an exponential moving average: eff = 0.9 * eff + 0.1 * feedback
    fn update_effectiveness(&mut self, strategy_id: u32, feedback: Q16) {
        if let Some(s) = self.strategies.iter_mut().find(|s| s.id == strategy_id) {
            s.uses += 1;

            // EMA weights in Q16: 0.9 ~= 58982, 0.1 ~= 6554
            let alpha = 6554i32; // 0.1 in Q16
            let one_minus_alpha = 58982i32; // 0.9 in Q16

            s.effectiveness = q16_mul(one_minus_alpha, s.effectiveness) + q16_mul(alpha, feedback);

            // Update average feedback: running mean
            if s.uses == 1 {
                s.avg_feedback = feedback;
            } else {
                s.avg_feedback =
                    q16_mul(one_minus_alpha, s.avg_feedback) + q16_mul(alpha, feedback);
            }

            // Propagate to evolution records
            self.update_evolution_fitness(strategy_id);
        }

        // Recompute global improvement rate
        self.recompute_improvement_rate();
    }

    /// After updating a child strategy, update the fitness_delta in the
    /// corresponding evolution record.
    fn update_evolution_fitness(&mut self, child_id: u32) {
        for evo in &mut self.evolutions {
            if evo.child_strategy == child_id {
                // Find parent effectiveness
                let parent_eff = self
                    .strategies
                    .iter()
                    .find(|s| s.id == evo.parent_strategy)
                    .map_or(q16_from_int(50), |s| s.effectiveness);
                let child_eff = self
                    .strategies
                    .iter()
                    .find(|s| s.id == child_id)
                    .map_or(q16_from_int(50), |s| s.effectiveness);
                evo.fitness_delta = child_eff - parent_eff;
            }
        }
    }

    /// Recompute the overall improvement rate.
    /// Defined as (best effectiveness - baseline 50) / generation, Q16.
    fn recompute_improvement_rate(&mut self) {
        let baseline = q16_from_int(50);
        let best_eff = self
            .strategies
            .iter()
            .map(|s| s.effectiveness)
            .max()
            .unwrap_or(baseline);

        let delta = best_eff - baseline;
        let gen = if self.generation > 0 {
            self.generation as i32
        } else {
            1
        };
        self.improvement_rate = delta / gen;
    }

    // ------------------------------------------------------------------
    // Stats
    // ------------------------------------------------------------------

    /// Return summary stats: (strategies, experiments, generations, improvement_rate)
    fn get_improvement_rate(&self) -> Q16 {
        self.improvement_rate
    }

    fn get_stats(&self) -> (u32, u32, u32, Q16) {
        (
            self.strategies.len() as u32,
            self.experiments.len() as u32,
            self.generation,
            self.improvement_rate,
        )
    }
}

// ---------------------------------------------------------------------------
// Public init
// ---------------------------------------------------------------------------

pub fn init() {
    let mut e = ENGINE.lock();
    *e = Some(SelfImproveEngine::new());
    serial_println!(
        "    Self-Improve: prompt strategy evolution, A/B experiments, reasoning selection ready"
    );
}

use crate::sync::Mutex;
/// Self-improving AI prompts for Genesis
///
/// Prompt template evolution, A/B testing, quality scoring,
/// and auto-refinement — all running on-device with Q16 fixed-point math.
///
/// The system tracks which prompt templates produce the best results,
/// evolves them over time, and automatically refines underperformers.
///
/// No data ever leaves the device. All improvement is local.
///
/// Inspired by: DSPy (prompt optimization), PromptBreeder. All code is original.
use crate::{serial_print, serial_println};
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

// ---------------------------------------------------------------------------
// Prompt template
// ---------------------------------------------------------------------------

/// Maximum number of template slots
const MAX_TEMPLATE_SLOTS: usize = 8;

/// Maximum templates tracked by the engine
const MAX_TEMPLATES: usize = 128;

/// Maximum A/B experiments active at once
const MAX_EXPERIMENTS: usize = 32;

/// Maximum refinement history entries
const MAX_HISTORY: usize = 256;

/// A slot inside a prompt template that can be filled dynamically
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotKind {
    SystemContext,
    UserQuery,
    FewShotExample,
    InstructionPrefix,
    OutputFormat,
    ChainOfThought,
    Constraint,
    Persona,
}

/// One fillable slot in a template
pub struct TemplateSlot {
    pub kind: SlotKind,
    pub content: String,
    pub required: bool,
    pub weight: i32, // Q16 importance weight
}

/// A complete prompt template
pub struct PromptTemplate {
    pub id: u32,
    pub name: String,
    pub version: u32,
    pub slots: Vec<TemplateSlot>,
    pub quality_score: i32, // Q16 running average
    pub usage_count: u64,
    pub success_count: u64,
    pub total_score_sum: i64, // accumulated Q16 scores
    pub created_at: u64,
    pub last_used: u64,
    pub active: bool,
    pub parent_id: Option<u32>,
}

impl PromptTemplate {
    /// Compute the success rate as Q16
    pub fn success_rate(&self) -> i32 {
        if self.usage_count == 0 {
            return 0;
        }
        (((self.success_count as i64) << 16) / (self.usage_count as i64)) as i32
    }

    /// Compute the average quality score as Q16
    pub fn avg_quality(&self) -> i32 {
        if self.usage_count == 0 {
            return 0;
        }
        (self.total_score_sum / (self.usage_count as i64)) as i32
    }

    /// Render the template by concatenating slot contents in order
    pub fn render(&self) -> String {
        let mut out = String::new();
        for slot in &self.slots {
            if !slot.content.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&slot.content);
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Quality scorer
// ---------------------------------------------------------------------------

/// Dimensions of quality we measure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityDimension {
    Relevance,
    Coherence,
    Completeness,
    Conciseness,
    Accuracy,
    Helpfulness,
}

/// A single quality assessment record
pub struct QualityAssessment {
    pub template_id: u32,
    pub dimension_scores: Vec<(QualityDimension, i32)>, // Q16
    pub composite_score: i32,                           // Q16
    pub timestamp: u64,
}

/// Compute a composite quality score from dimension scores with weights
fn compute_composite(dims: &[(QualityDimension, i32)]) -> i32 {
    if dims.is_empty() {
        return 0;
    }
    let weights: [(QualityDimension, i32); 6] = [
        (QualityDimension::Relevance, q16_from_int(3)),
        (QualityDimension::Coherence, q16_from_int(2)),
        (QualityDimension::Completeness, q16_from_int(2)),
        (QualityDimension::Conciseness, q16_from_int(1)),
        (QualityDimension::Accuracy, q16_from_int(3)),
        (QualityDimension::Helpfulness, q16_from_int(2)),
    ];
    let mut weighted_sum: i64 = 0;
    let mut weight_total: i64 = 0;
    for (dim, score) in dims {
        for (wd, wv) in &weights {
            if wd == dim {
                weighted_sum += (q16_mul(*score, *wv)) as i64;
                weight_total += *wv as i64;
                break;
            }
        }
    }
    if weight_total == 0 {
        return 0;
    }
    ((weighted_sum << 16) / weight_total) as i32
}

// ---------------------------------------------------------------------------
// A/B experiment
// ---------------------------------------------------------------------------

/// State of an A/B experiment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExperimentState {
    Running,
    ConcludedAWins,
    ConcludedBWins,
    ConcludedTie,
    Cancelled,
}

/// An A/B test between two prompt templates
pub struct AbExperiment {
    pub id: u32,
    pub template_a: u32,
    pub template_b: u32,
    pub state: ExperimentState,
    pub a_scores: Vec<i32>, // Q16 scores per trial
    pub b_scores: Vec<i32>,
    pub min_trials: u32,
    pub confidence_threshold: i32, // Q16 — minimum difference to declare winner
    pub started_at: u64,
}

impl AbExperiment {
    /// Mean of collected scores (Q16)
    fn mean(scores: &[i32]) -> i32 {
        if scores.is_empty() {
            return 0;
        }
        let sum: i64 = scores.iter().map(|s| *s as i64).sum();
        (sum / (scores.len() as i64)) as i32
    }

    /// Variance of scores (Q16, approximate)
    fn variance(scores: &[i32]) -> i32 {
        if scores.len() < 2 {
            return 0;
        }
        let m = Self::mean(scores);
        let sum_sq: i64 = scores
            .iter()
            .map(|s| {
                let diff = (*s as i64) - (m as i64);
                (diff * diff) >> 16 // keep in Q16 range
            })
            .sum();
        (sum_sq / ((scores.len() - 1) as i64)) as i32
    }

    /// Check whether the experiment can be concluded
    pub fn check_conclusion(&mut self) -> ExperimentState {
        if (self.a_scores.len() as u32) < self.min_trials
            || (self.b_scores.len() as u32) < self.min_trials
        {
            return ExperimentState::Running;
        }
        let mean_a = Self::mean(&self.a_scores);
        let mean_b = Self::mean(&self.b_scores);
        let diff = (mean_a as i64 - mean_b as i64).unsigned_abs() as i32;

        if diff < self.confidence_threshold {
            self.state = ExperimentState::ConcludedTie;
        } else if mean_a > mean_b {
            self.state = ExperimentState::ConcludedAWins;
        } else {
            self.state = ExperimentState::ConcludedBWins;
        }
        self.state
    }
}

// ---------------------------------------------------------------------------
// Refinement record
// ---------------------------------------------------------------------------

/// What kind of refinement was applied
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefinementKind {
    SlotReorder,
    SlotWeightTune,
    SlotContentTweak,
    TemplateClone,
    TemplateMerge,
    TemplateRetire,
}

/// History entry for a refinement action
pub struct RefinementRecord {
    pub template_id: u32,
    pub kind: RefinementKind,
    pub old_score: i32, // Q16
    pub new_score: i32, // Q16
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Self-improvement engine
// ---------------------------------------------------------------------------

/// The main self-improvement engine
pub struct SelfImproveEngine {
    pub templates: Vec<PromptTemplate>,
    pub experiments: Vec<AbExperiment>,
    pub history: Vec<RefinementRecord>,
    pub assessments: Vec<QualityAssessment>,
    pub next_template_id: u32,
    pub next_experiment_id: u32,
    pub auto_refine_enabled: bool,
    pub retire_threshold: i32, // Q16 — templates below this get retired
    pub evolve_threshold: i32, // Q16 — templates above this get cloned/evolved
    pub total_refinements: u64,
}

impl SelfImproveEngine {
    const fn new() -> Self {
        SelfImproveEngine {
            templates: Vec::new(),
            experiments: Vec::new(),
            history: Vec::new(),
            assessments: Vec::new(),
            next_template_id: 1,
            next_experiment_id: 1,
            auto_refine_enabled: true,
            retire_threshold: Q16_ONE / 4,       // 0.25 in Q16
            evolve_threshold: (Q16_ONE * 3) / 4, // 0.75 in Q16
            total_refinements: 0,
        }
    }

    /// Create a new prompt template
    pub fn create_template(&mut self, name: &str, slots: Vec<TemplateSlot>) -> u32 {
        let id = self.next_template_id;
        self.next_template_id = self.next_template_id.saturating_add(1);
        let now = crate::time::clock::unix_time();
        self.templates.push(PromptTemplate {
            id,
            name: String::from(name),
            version: 1,
            slots,
            quality_score: Q16_ONE / 2, // start at 0.5
            usage_count: 0,
            success_count: 0,
            total_score_sum: 0,
            created_at: now,
            last_used: now,
            active: true,
            parent_id: None,
        });
        if self.templates.len() > MAX_TEMPLATES {
            self.prune_weakest();
        }
        id
    }

    /// Record a quality assessment for a template
    pub fn record_assessment(&mut self, template_id: u32, dims: Vec<(QualityDimension, i32)>) {
        let composite = compute_composite(&dims);
        let now = crate::time::clock::unix_time();
        if let Some(tpl) = self.templates.iter_mut().find(|t| t.id == template_id) {
            tpl.usage_count = tpl.usage_count.saturating_add(1);
            tpl.last_used = now;
            tpl.total_score_sum += composite as i64;
            tpl.quality_score = tpl.avg_quality();
            if composite >= Q16_ONE / 2 {
                tpl.success_count = tpl.success_count.saturating_add(1);
            }
        }
        self.assessments.push(QualityAssessment {
            template_id,
            dimension_scores: dims,
            composite_score: composite,
            timestamp: now,
        });
        if self.assessments.len() > MAX_HISTORY {
            self.assessments.remove(0);
        }
    }

    /// Start an A/B experiment between two templates
    pub fn start_experiment(&mut self, a: u32, b: u32, min_trials: u32) -> u32 {
        let id = self.next_experiment_id;
        self.next_experiment_id = self.next_experiment_id.saturating_add(1);
        let now = crate::time::clock::unix_time();
        self.experiments.push(AbExperiment {
            id,
            template_a: a,
            template_b: b,
            state: ExperimentState::Running,
            a_scores: Vec::new(),
            b_scores: Vec::new(),
            min_trials,
            confidence_threshold: Q16_ONE / 10, // 0.1 in Q16
            started_at: now,
        });
        if self.experiments.len() > MAX_EXPERIMENTS {
            if let Some(pos) = self
                .experiments
                .iter()
                .position(|e| e.state != ExperimentState::Running)
            {
                self.experiments.remove(pos);
            }
        }
        id
    }

    /// Record a trial score for an experiment
    pub fn record_trial(&mut self, experiment_id: u32, is_variant_b: bool, score: i32) {
        if let Some(exp) = self.experiments.iter_mut().find(|e| e.id == experiment_id) {
            if exp.state != ExperimentState::Running {
                return;
            }
            if is_variant_b {
                exp.b_scores.push(score);
            } else {
                exp.a_scores.push(score);
            }
            exp.check_conclusion();
        }
    }

    /// Clone a template to create a child variant for evolution
    pub fn evolve_template(&mut self, parent_id: u32, new_name: &str) -> Option<u32> {
        let parent = self.templates.iter().find(|t| t.id == parent_id)?;
        let slots: Vec<TemplateSlot> = parent
            .slots
            .iter()
            .map(|s| TemplateSlot {
                kind: s.kind,
                content: s.content.clone(),
                required: s.required,
                weight: s.weight,
            })
            .collect();
        let version = parent.version + 1;
        let id = self.next_template_id;
        self.next_template_id = self.next_template_id.saturating_add(1);
        let now = crate::time::clock::unix_time();
        self.templates.push(PromptTemplate {
            id,
            name: String::from(new_name),
            version,
            slots,
            quality_score: Q16_ONE / 2,
            usage_count: 0,
            success_count: 0,
            total_score_sum: 0,
            created_at: now,
            last_used: now,
            active: true,
            parent_id: Some(parent_id),
        });
        Some(id)
    }

    /// Retire a template (mark inactive)
    pub fn retire_template(&mut self, template_id: u32) {
        if let Some(tpl) = self.templates.iter_mut().find(|t| t.id == template_id) {
            tpl.active = false;
            let now = crate::time::clock::unix_time();
            self.history.push(RefinementRecord {
                template_id,
                kind: RefinementKind::TemplateRetire,
                old_score: tpl.quality_score,
                new_score: 0,
                timestamp: now,
            });
            self.total_refinements = self.total_refinements.saturating_add(1);
        }
    }

    /// Adjust slot weights for a template based on quality dimension feedback
    pub fn tune_slot_weights(&mut self, template_id: u32) {
        let tpl = match self.templates.iter_mut().find(|t| t.id == template_id) {
            Some(t) => t,
            None => return,
        };
        let old_score = tpl.quality_score;

        // Boost weights of required slots and slightly decay optional ones
        for slot in &mut tpl.slots {
            if slot.required {
                slot.weight = slot.weight.saturating_add(Q16_ONE / 20); // +0.05
            } else {
                // Slight decay toward baseline
                let decay = q16_mul(slot.weight, Q16_ONE / 100); // 1% decay
                slot.weight = slot.weight.saturating_sub(decay);
            }
        }

        let now = crate::time::clock::unix_time();
        self.history.push(RefinementRecord {
            template_id,
            kind: RefinementKind::SlotWeightTune,
            old_score,
            new_score: tpl.quality_score,
            timestamp: now,
        });
        self.total_refinements = self.total_refinements.saturating_add(1);
        if self.history.len() > MAX_HISTORY {
            self.history.remove(0);
        }
    }

    /// Run the auto-refinement cycle: retire bad, evolve good, conclude experiments
    pub fn auto_refine(&mut self) {
        if !self.auto_refine_enabled {
            return;
        }

        // 1. Conclude ready experiments
        for exp in &mut self.experiments {
            if exp.state == ExperimentState::Running {
                exp.check_conclusion();
            }
        }

        // 2. Retire underperformers (only if they have enough usage)
        let min_usage: u64 = 10;
        let retire_ids: Vec<u32> = self
            .templates
            .iter()
            .filter(|t| {
                t.active && t.usage_count >= min_usage && t.quality_score < self.retire_threshold
            })
            .map(|t| t.id)
            .collect();
        for id in retire_ids {
            self.retire_template(id);
        }

        // 3. Evolve top performers
        let evolve_ids: Vec<u32> = self
            .templates
            .iter()
            .filter(|t| {
                t.active && t.usage_count >= min_usage && t.quality_score > self.evolve_threshold
            })
            .map(|t| t.id)
            .collect();
        for id in evolve_ids {
            let name = alloc::format!("evolved_{}", id);
            if let Some(child_id) = self.evolve_template(id, &name) {
                self.start_experiment(id, child_id, 20);
            }
        }

        // 4. Tune slot weights on mid-range templates
        let tune_ids: Vec<u32> = self
            .templates
            .iter()
            .filter(|t| {
                t.active
                    && t.usage_count >= min_usage
                    && t.quality_score >= self.retire_threshold
                    && t.quality_score <= self.evolve_threshold
            })
            .map(|t| t.id)
            .collect();
        for id in tune_ids {
            self.tune_slot_weights(id);
        }
    }

    /// Remove the weakest inactive template to stay under MAX_TEMPLATES
    fn prune_weakest(&mut self) {
        if let Some(pos) = self
            .templates
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.active)
            .min_by_key(|(_, t)| t.quality_score)
            .map(|(i, _)| i)
        {
            self.templates.remove(pos);
        }
    }

    /// Get the best active template by quality score
    pub fn best_template(&self) -> Option<&PromptTemplate> {
        self.templates
            .iter()
            .filter(|t| t.active)
            .max_by_key(|t| t.quality_score)
    }

    /// Count active templates
    pub fn active_count(&self) -> usize {
        self.templates.iter().filter(|t| t.active).count()
    }

    /// Count running experiments
    pub fn running_experiments(&self) -> usize {
        self.experiments
            .iter()
            .filter(|e| e.state == ExperimentState::Running)
            .count()
    }

    /// Get refinement statistics
    pub fn refinement_stats(&self) -> (u64, usize, usize) {
        (
            self.total_refinements,
            self.active_count(),
            self.running_experiments(),
        )
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SELF_IMPROVE: Mutex<Option<SelfImproveEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = SelfImproveEngine::new();

    // Seed with a default "general assistant" template
    let slots = vec![
        TemplateSlot {
            kind: SlotKind::Persona,
            content: String::from("You are a helpful on-device assistant."),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::SystemContext,
            content: String::from("Answer concisely and accurately."),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::InstructionPrefix,
            content: String::from("User question:"),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::UserQuery,
            content: String::new(),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::OutputFormat,
            content: String::from("Respond in plain text."),
            required: false,
            weight: Q16_ONE / 2,
        },
    ];
    engine.create_template("general_assistant", slots);

    // Seed with a "code helper" template
    let code_slots = vec![
        TemplateSlot {
            kind: SlotKind::Persona,
            content: String::from("You are a code assistant for Genesis OS."),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::SystemContext,
            content: String::from("Provide correct, safe Rust code."),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::ChainOfThought,
            content: String::from("Think step by step before answering."),
            required: false,
            weight: Q16_ONE * 3 / 4,
        },
        TemplateSlot {
            kind: SlotKind::UserQuery,
            content: String::new(),
            required: true,
            weight: Q16_ONE,
        },
        TemplateSlot {
            kind: SlotKind::Constraint,
            content: String::from("No unsafe code unless absolutely necessary."),
            required: false,
            weight: Q16_ONE / 2,
        },
    ];
    engine.create_template("code_helper", code_slots);

    *SELF_IMPROVE.lock() = Some(engine);
    serial_println!(
        "    [self_improve] Self-improving prompt engine initialized (2 seed templates)"
    );
}

/// Create a new template
pub fn create_template(name: &str, slots: Vec<TemplateSlot>) -> u32 {
    SELF_IMPROVE
        .lock()
        .as_mut()
        .map(|e| e.create_template(name, slots))
        .unwrap_or(0)
}

/// Record a quality assessment
pub fn record_assessment(template_id: u32, dims: Vec<(QualityDimension, i32)>) {
    if let Some(engine) = SELF_IMPROVE.lock().as_mut() {
        engine.record_assessment(template_id, dims);
    }
}

/// Run the auto-refinement cycle
pub fn auto_refine() {
    if let Some(engine) = SELF_IMPROVE.lock().as_mut() {
        engine.auto_refine();
    }
}

/// Get refinement statistics: (total_refinements, active_templates, running_experiments)
pub fn stats() -> (u64, usize, usize) {
    SELF_IMPROVE
        .lock()
        .as_ref()
        .map(|e| e.refinement_stats())
        .unwrap_or((0, 0, 0))
}

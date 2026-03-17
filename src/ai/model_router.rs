use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// Route queries to best model (small/medium/large)
///
/// Part of the AIOS AI layer. Registry of local model backends with
/// capability tags, latency estimates, and memory requirements.
/// Selects the best model for a task based on query complexity,
/// required capabilities, and resource constraints.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Available model size tiers
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Small,
    Medium,
    Large,
}

/// Capability tags that a model can support
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    TextGeneration,
    Classification,
    Summarization,
    CodeGeneration,
    QuestionAnswering,
    Sentiment,
    Translation,
    Embedding,
    Reasoning,
    Math,
    Custom(String),
}

/// A registered model backend
#[derive(Clone)]
pub struct ModelBackend {
    pub name: String,
    pub tier: ModelTier,
    pub capabilities: Vec<Capability>,
    /// Estimated latency in milliseconds per token
    pub latency_ms_per_token: u32,
    /// Memory requirement in megabytes
    pub memory_mb: u32,
    /// Maximum context length in tokens
    pub max_context: u32,
    /// Quality score (0.0 to 1.0, higher is better)
    pub quality_score: f32,
    /// Whether this model is currently available (loaded)
    pub available: bool,
    /// Request count for load tracking
    pub request_count: u64,
    /// Cumulative latency for average calculation
    pub total_latency_ms: u64,
}

impl ModelBackend {
    fn new(name: &str, tier: ModelTier) -> Self {
        ModelBackend {
            name: String::from(name),
            tier,
            capabilities: Vec::new(),
            latency_ms_per_token: 10,
            memory_mb: 256,
            max_context: 4096,
            quality_score: 0.5,
            available: true,
            request_count: 0,
            total_latency_ms: 0,
        }
    }

    fn with_capabilities(mut self, caps: Vec<Capability>) -> Self {
        self.capabilities = caps;
        self
    }

    fn with_specs(mut self, latency: u32, memory: u32, context: u32, quality: f32) -> Self {
        self.latency_ms_per_token = latency;
        self.memory_mb = memory;
        self.max_context = context;
        self.quality_score = quality;
        self
    }

    /// Check if this model has a specific capability
    fn has_capability(&self, cap: &Capability) -> bool {
        self.capabilities.iter().any(|c| c == cap)
    }

    /// Average observed latency (if any requests have been served)
    fn avg_latency(&self) -> u32 {
        if self.request_count == 0 {
            self.latency_ms_per_token
        } else {
            (self.total_latency_ms / self.request_count) as u32
        }
    }
}

/// Routing constraints
pub struct RoutingConstraints {
    /// Maximum acceptable latency per token (ms)
    pub max_latency_ms: Option<u32>,
    /// Maximum memory budget (MB)
    pub max_memory_mb: Option<u32>,
    /// Minimum required context length
    pub min_context: Option<u32>,
    /// Required capabilities
    pub required_capabilities: Vec<Capability>,
    /// Prefer quality over speed
    pub prefer_quality: bool,
}

impl RoutingConstraints {
    pub fn default_constraints() -> Self {
        RoutingConstraints {
            max_latency_ms: None,
            max_memory_mb: None,
            min_context: None,
            required_capabilities: Vec::new(),
            prefer_quality: false,
        }
    }

    pub fn fast() -> Self {
        RoutingConstraints {
            max_latency_ms: Some(5),
            max_memory_mb: Some(512),
            min_context: None,
            required_capabilities: Vec::new(),
            prefer_quality: false,
        }
    }

    pub fn quality() -> Self {
        RoutingConstraints {
            max_latency_ms: None,
            max_memory_mb: None,
            min_context: None,
            required_capabilities: Vec::new(),
            prefer_quality: true,
        }
    }
}

/// Routes queries to the most appropriate model tier
pub struct ModelRouter {
    pub default_tier: ModelTier,
    /// Registry of available model backends
    models: Vec<ModelBackend>,
    /// Complexity thresholds: (max_complexity_for_small, max_for_medium)
    complexity_thresholds: (u32, u32),
    /// Available system memory (MB) for resource-aware routing
    available_memory_mb: u32,
    /// Total requests routed
    total_requests: u64,
    /// Per-tier request counts
    tier_counts: BTreeMap<u8, u64>,
}

impl ModelRouter {
    pub fn new() -> Self {
        ModelRouter {
            default_tier: ModelTier::Small,
            models: Vec::new(),
            complexity_thresholds: (30, 70),
            available_memory_mb: 4096,
            total_requests: 0,
            tier_counts: BTreeMap::new(),
        }
    }

    /// Register a model backend
    pub fn register_model(&mut self, model: ModelBackend) {
        self.models.push(model);
    }

    /// Set available system memory for routing decisions
    pub fn set_available_memory(&mut self, mb: u32) {
        self.available_memory_mb = mb;
    }

    /// Set complexity thresholds (small_max, medium_max)
    pub fn set_thresholds(&mut self, small_max: u32, medium_max: u32) {
        self.complexity_thresholds = (small_max, medium_max);
    }

    /// Route a query to the best model tier based on complexity
    pub fn route(&self, query: &str) -> ModelTier {
        let complexity = self.estimate_complexity(query);
        let (small_max, medium_max) = self.complexity_thresholds;

        if complexity <= small_max {
            ModelTier::Small
        } else if complexity <= medium_max {
            ModelTier::Medium
        } else {
            ModelTier::Large
        }
    }

    /// Route with constraints, returning the best matching model
    pub fn route_constrained(
        &self,
        query: &str,
        constraints: &RoutingConstraints,
    ) -> Option<&ModelBackend> {
        let mut candidates: Vec<(usize, f32)> = Vec::new();

        for (idx, model) in self.models.iter().enumerate() {
            if !model.available {
                continue;
            }

            // Check hard constraints
            if let Some(max_lat) = constraints.max_latency_ms {
                if model.avg_latency() > max_lat {
                    continue;
                }
            }
            if let Some(max_mem) = constraints.max_memory_mb {
                if model.memory_mb > max_mem {
                    continue;
                }
            }
            if let Some(min_ctx) = constraints.min_context {
                if model.max_context < min_ctx {
                    continue;
                }
            }
            if model.memory_mb > self.available_memory_mb {
                continue;
            }

            // Check required capabilities
            let has_all_caps = constraints
                .required_capabilities
                .iter()
                .all(|cap| model.has_capability(cap));
            if !has_all_caps {
                continue;
            }

            // Score the candidate
            let score = self.score_model(model, query, constraints);
            candidates.push((idx, score));
        }

        // Sort by score descending
        candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

        candidates
            .first()
            .and_then(|(idx, _)| self.models.get(*idx))
    }

    /// Score a model for a given query and constraints
    fn score_model(
        &self,
        model: &ModelBackend,
        query: &str,
        constraints: &RoutingConstraints,
    ) -> f32 {
        let complexity = self.estimate_complexity(query);
        let mut score = 0.0f32;

        // Quality component
        score += model.quality_score * 40.0;

        // Capability match: bonus for each capability that matches the query type
        let query_caps = infer_capabilities(query);
        let cap_match = query_caps
            .iter()
            .filter(|c| model.has_capability(c))
            .count();
        score += cap_match as f32 * 10.0;

        // Tier appropriateness: penalize over/under-powered models
        let tier_score = match (model.tier, complexity) {
            (ModelTier::Small, c) if c <= 30 => 20.0,
            (ModelTier::Small, c) if c <= 50 => 10.0,
            (ModelTier::Small, _) => 0.0,
            (ModelTier::Medium, c) if c > 20 && c <= 70 => 20.0,
            (ModelTier::Medium, _) => 10.0,
            (ModelTier::Large, c) if c > 50 => 20.0,
            (ModelTier::Large, _) => 5.0,
        };
        score += tier_score;

        // Speed bonus (inverse of latency)
        if !constraints.prefer_quality {
            let speed_score = 20.0 / (1.0 + model.avg_latency() as f32 / 10.0);
            score += speed_score;
        }

        // Memory efficiency bonus
        let mem_score =
            10.0 * (1.0 - model.memory_mb as f32 / self.available_memory_mb.max(1) as f32);
        score += mem_score.max(0.0);

        score
    }

    /// Estimate query complexity (0-100 scale)
    pub fn estimate_complexity(&self, query: &str) -> u32 {
        let mut score = 0u32;

        // Length factor: longer queries tend to be more complex
        let word_count = query.split_whitespace().count();
        score += (word_count as u32 * 2).min(30);

        // Sentence count factor
        let sentence_count = query
            .bytes()
            .filter(|&b| b == b'.' || b == b'?' || b == b'!')
            .count();
        score += (sentence_count as u32 * 5).min(15);

        // Complexity keywords
        let complex_keywords = [
            "explain",
            "analyze",
            "compare",
            "contrast",
            "evaluate",
            "synthesize",
            "design",
            "implement",
            "optimize",
            "debug",
            "architecture",
            "algorithm",
            "strategy",
            "comprehensive",
            "detailed",
            "multi-step",
            "complex",
            "advanced",
        ];
        let reasoning_keywords = [
            "why",
            "how",
            "because",
            "therefore",
            "consequently",
            "implies",
            "reason",
            "logic",
            "proof",
            "derive",
        ];
        let code_keywords = [
            "code",
            "function",
            "class",
            "implement",
            "program",
            "compile",
            "debug",
            "refactor",
            "test",
            "api",
        ];
        let math_keywords = [
            "calculate",
            "compute",
            "formula",
            "equation",
            "integral",
            "derivative",
            "matrix",
            "probability",
            "statistics",
        ];

        let lower = query.to_lowercase();
        for kw in &complex_keywords {
            if lower.contains(kw) {
                score += 5;
            }
        }
        for kw in &reasoning_keywords {
            if lower.contains(kw) {
                score += 7;
            }
        }
        for kw in &code_keywords {
            if lower.contains(kw) {
                score += 6;
            }
        }
        for kw in &math_keywords {
            if lower.contains(kw) {
                score += 8;
            }
        }

        // Question complexity: compound questions
        let question_marks = query.bytes().filter(|&b| b == b'?').count();
        if question_marks > 1 {
            score += (question_marks as u32 - 1) * 5;
        }

        // Nested structure (parentheses, quotes)
        let parens = query.bytes().filter(|&b| b == b'(' || b == b')').count();
        score += (parens as u32 * 2).min(10);

        score.min(100)
    }

    /// Record that a request was served by a specific model
    pub fn record_request(&mut self, model_name: &str, latency_ms: u64) {
        self.total_requests = self.total_requests.saturating_add(1);
        for model in &mut self.models {
            if model.name == model_name {
                model.request_count = model.request_count.saturating_add(1);
                model.total_latency_ms = model.total_latency_ms.saturating_add(latency_ms);
                let tier_key = match model.tier {
                    ModelTier::Small => 0u8,
                    ModelTier::Medium => 1,
                    ModelTier::Large => 2,
                };
                let cnt = self.tier_counts.entry(tier_key).or_insert(0);
                *cnt = cnt.saturating_add(1);
                break;
            }
        }
    }

    /// Set a model's availability
    pub fn set_available(&mut self, model_name: &str, available: bool) {
        for model in &mut self.models {
            if model.name == model_name {
                model.available = available;
                break;
            }
        }
    }

    /// Get all registered models
    pub fn list_models(&self) -> &[ModelBackend] {
        &self.models
    }

    /// Get models for a specific tier
    pub fn models_for_tier(&self, tier: ModelTier) -> Vec<&ModelBackend> {
        self.models.iter().filter(|m| m.tier == tier).collect()
    }

    /// Get total requests routed
    pub fn total_requests(&self) -> u64 {
        self.total_requests
    }

    /// Get routing distribution as (small%, medium%, large%)
    pub fn routing_distribution(&self) -> (f32, f32, f32) {
        if self.total_requests == 0 {
            return (0.0, 0.0, 0.0);
        }
        let total = self.total_requests as f32;
        let small = self.tier_counts.get(&0).copied().unwrap_or(0) as f32 / total;
        let medium = self.tier_counts.get(&1).copied().unwrap_or(0) as f32 / total;
        let large = self.tier_counts.get(&2).copied().unwrap_or(0) as f32 / total;
        (small, medium, large)
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Infer what capabilities a query likely needs
fn infer_capabilities(query: &str) -> Vec<Capability> {
    let lower = query.to_lowercase();
    let mut caps = Vec::new();

    if lower.contains("summariz") || lower.contains("summary") || lower.contains("tldr") {
        caps.push(Capability::Summarization);
    }
    if lower.contains("classif") || lower.contains("categoriz") || lower.contains("label") {
        caps.push(Capability::Classification);
    }
    if lower.contains("code")
        || lower.contains("program")
        || lower.contains("function")
        || lower.contains("implement")
    {
        caps.push(Capability::CodeGeneration);
    }
    if lower.contains("sentiment") || lower.contains("feeling") || lower.contains("emotion") {
        caps.push(Capability::Sentiment);
    }
    if lower.contains("translat") {
        caps.push(Capability::Translation);
    }
    if lower.contains("embed") || lower.contains("vector") || lower.contains("similar") {
        caps.push(Capability::Embedding);
    }
    if lower.contains("reason")
        || lower.contains("logic")
        || lower.contains("deduc")
        || lower.contains("infer")
    {
        caps.push(Capability::Reasoning);
    }
    if lower.contains("calculat")
        || lower.contains("math")
        || lower.contains("equation")
        || lower.contains("formula")
    {
        caps.push(Capability::Math);
    }
    if lower.contains("?")
        || lower.contains("what")
        || lower.contains("how")
        || lower.contains("why")
        || lower.contains("when")
        || lower.contains("where")
    {
        caps.push(Capability::QuestionAnswering);
    }

    // Default to text generation if no specific capability matched
    if caps.is_empty() {
        caps.push(Capability::TextGeneration);
    }

    caps
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ROUTER: Mutex<Option<ModelRouter>> = Mutex::new(None);

pub fn init() {
    let mut router = ModelRouter::new();

    // Register default model backends
    router.register_model(
        ModelBackend::new("hoags-tiny-125M", ModelTier::Small)
            .with_capabilities(alloc::vec![
                Capability::TextGeneration,
                Capability::Classification,
                Capability::Sentiment,
                Capability::Embedding,
            ])
            .with_specs(2, 128, 2048, 0.4),
    );

    router.register_model(
        ModelBackend::new("hoags-small-350M", ModelTier::Small)
            .with_capabilities(alloc::vec![
                Capability::TextGeneration,
                Capability::Classification,
                Capability::Summarization,
                Capability::Sentiment,
                Capability::QuestionAnswering,
                Capability::Embedding,
            ])
            .with_specs(5, 256, 4096, 0.55),
    );

    router.register_model(
        ModelBackend::new("hoags-medium-1B", ModelTier::Medium)
            .with_capabilities(alloc::vec![
                Capability::TextGeneration,
                Capability::Classification,
                Capability::Summarization,
                Capability::CodeGeneration,
                Capability::QuestionAnswering,
                Capability::Sentiment,
                Capability::Reasoning,
                Capability::Embedding,
            ])
            .with_specs(12, 512, 8192, 0.7),
    );

    router.register_model(
        ModelBackend::new("hoags-large-7B", ModelTier::Large)
            .with_capabilities(alloc::vec![
                Capability::TextGeneration,
                Capability::Classification,
                Capability::Summarization,
                Capability::CodeGeneration,
                Capability::QuestionAnswering,
                Capability::Sentiment,
                Capability::Translation,
                Capability::Reasoning,
                Capability::Math,
                Capability::Embedding,
            ])
            .with_specs(30, 4096, 32768, 0.9),
    );

    *ROUTER.lock() = Some(router);
    crate::serial_println!("    [model_router] Model router ready (4 backends, S/M/L tiers)");
}

/// Route a query to the appropriate model tier
pub fn route(query: &str) -> ModelTier {
    ROUTER
        .lock()
        .as_ref()
        .map(|r| r.route(query))
        .unwrap_or(ModelTier::Small)
}

/// Estimate query complexity
pub fn estimate_complexity(query: &str) -> u32 {
    ROUTER
        .lock()
        .as_ref()
        .map(|r| r.estimate_complexity(query))
        .unwrap_or(0)
}

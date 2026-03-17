/// Safety / Alignment Layer — user-controlled content filtering
///
/// This is NOT corporate censorship. This is the owner's control
/// panel for their own AI. The user decides what the AI can and
/// cannot do. Full uncensored mode is a first-class option.
///
/// The Hoags AI is YOUR property running on YOUR hardware.
/// You set the rules. You decide the boundaries. You control
/// whether filtering is on or off, strict or permissive.
///
/// Features:
///   - Configurable content filtering (off by default)
///   - User-defined guardrails (topic blocks, output limits)
///   - Uncensored mode toggle (no restrictions)
///   - Content category scoring and thresholds
///   - Output validation against user rules
///   - Audit log of all safety decisions
///   - Per-category enable/disable controls
///   - Allowlist and blocklist for content patterns

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

use super::transformer::{Q16, q16_mul, q16_from_int};

// ── Constants ────────────────────────────────────────────────────────

/// Maximum number of user-defined rules
const MAX_USER_RULES: usize = 128;

/// Maximum audit log entries
const MAX_AUDIT_LOG: usize = 2048;

/// Maximum allowlist/blocklist entries
const MAX_LIST_ENTRIES: usize = 256;

/// Default threshold for flagging (0.7 in Q16 = 45875)
const DEFAULT_THRESHOLD: Q16 = 45875;

/// Number of content categories
const CATEGORY_COUNT: usize = 10;

// ── Types ────────────────────────────────────────────────────────────

/// Content categories the user can choose to filter
#[derive(Clone, Copy, PartialEq)]
pub enum ContentCategory {
    Violence,
    AdultContent,
    PoliticalOpinion,
    MedicalAdvice,
    LegalAdvice,
    FinancialAdvice,
    PersonalInfo,
    Profanity,
    Misinformation,
    DangerousInfo,
}

/// Map a category to its index
fn category_index(cat: ContentCategory) -> usize {
    match cat {
        ContentCategory::Violence => 0,
        ContentCategory::AdultContent => 1,
        ContentCategory::PoliticalOpinion => 2,
        ContentCategory::MedicalAdvice => 3,
        ContentCategory::LegalAdvice => 4,
        ContentCategory::FinancialAdvice => 5,
        ContentCategory::PersonalInfo => 6,
        ContentCategory::Profanity => 7,
        ContentCategory::Misinformation => 8,
        ContentCategory::DangerousInfo => 9,
    }
}

/// All categories for iteration
fn all_categories() -> [ContentCategory; CATEGORY_COUNT] {
    [
        ContentCategory::Violence,
        ContentCategory::AdultContent,
        ContentCategory::PoliticalOpinion,
        ContentCategory::MedicalAdvice,
        ContentCategory::LegalAdvice,
        ContentCategory::FinancialAdvice,
        ContentCategory::PersonalInfo,
        ContentCategory::Profanity,
        ContentCategory::Misinformation,
        ContentCategory::DangerousInfo,
    ]
}

/// What action to take when content is flagged
#[derive(Clone, Copy, PartialEq)]
pub enum FilterAction {
    Allow,          // Let it through
    Warn,           // Add a disclaimer
    Block,          // Suppress the content
    LogOnly,        // Allow but log for review
}

/// Configuration for a single content category
#[derive(Clone, Copy)]
pub struct CategoryConfig {
    pub category: ContentCategory,
    pub enabled: bool,
    pub threshold: Q16,
    pub action: FilterAction,
    pub triggered_count: u64,
}

impl CategoryConfig {
    fn new(category: ContentCategory) -> Self {
        CategoryConfig {
            category,
            enabled: false, // Off by default — user must opt in
            threshold: DEFAULT_THRESHOLD,
            action: FilterAction::Allow,
            triggered_count: 0,
        }
    }
}

/// The overall safety mode
#[derive(Clone, Copy, PartialEq)]
pub enum SafetyMode {
    /// No filtering at all — full uncensored AI
    Uncensored,
    /// Minimal filtering — only user-defined blocklist
    Minimal,
    /// Standard filtering — user-chosen categories active
    Standard,
    /// Strict filtering — all categories active with blocking
    Strict,
    /// Custom — per-category user configuration
    Custom,
}

/// A user-defined guardrail rule
#[derive(Clone, Copy)]
pub struct UserRule {
    pub id: u32,
    pub pattern_hash: u64,
    pub action: FilterAction,
    pub is_allowlist: bool,
    pub enabled: bool,
    pub match_count: u64,
}

/// Audit log entry — records every safety decision
#[derive(Clone, Copy)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub content_hash: u64,
    pub category: ContentCategory,
    pub score: Q16,
    pub action_taken: FilterAction,
    pub rule_id: u32,
    pub overridden: bool,
}

/// Result of content evaluation
#[derive(Clone, Copy)]
pub struct EvalResult {
    pub allowed: bool,
    pub action: FilterAction,
    pub category: ContentCategory,
    pub score: Q16,
    pub rule_matched: u32,
}

// ── Safety Engine ────────────────────────────────────────────────────

struct SafetyEngine {
    mode: SafetyMode,
    categories: Vec<CategoryConfig>,
    user_rules: Vec<UserRule>,
    allowlist: Vec<u64>,
    blocklist: Vec<u64>,
    audit_log: Vec<AuditEntry>,
    next_rule_id: u32,
    total_evaluations: u64,
    total_blocked: u64,
    total_warned: u64,
    total_allowed: u64,
}

impl SafetyEngine {
    fn new() -> Self {
        // Initialize all categories as disabled (uncensored by default)
        let mut categories = Vec::with_capacity(CATEGORY_COUNT);
        for cat in all_categories().iter() {
            categories.push(CategoryConfig::new(*cat));
        }

        SafetyEngine {
            mode: SafetyMode::Uncensored,
            categories,
            user_rules: Vec::new(),
            allowlist: Vec::new(),
            blocklist: Vec::new(),
            audit_log: Vec::new(),
            next_rule_id: 1,
            total_evaluations: 0,
            total_blocked: 0,
            total_warned: 0,
            total_allowed: 0,
        }
    }

    // ── Mode Control ─────────────────────────────────────────────────

    /// Set the safety mode — this is the master switch
    fn set_mode(&mut self, mode: SafetyMode) {
        self.mode = mode;

        match mode {
            SafetyMode::Uncensored => {
                // Disable everything
                for cat in &mut self.categories {
                    cat.enabled = false;
                    cat.action = FilterAction::Allow;
                }
            }
            SafetyMode::Minimal => {
                // Only blocklist active, all categories disabled
                for cat in &mut self.categories {
                    cat.enabled = false;
                    cat.action = FilterAction::Allow;
                }
            }
            SafetyMode::Standard => {
                // Enable common categories with warn action
                for cat in &mut self.categories {
                    cat.enabled = true;
                    cat.action = FilterAction::Warn;
                    cat.threshold = DEFAULT_THRESHOLD;
                }
            }
            SafetyMode::Strict => {
                // Enable all categories with block action
                for cat in &mut self.categories {
                    cat.enabled = true;
                    cat.action = FilterAction::Block;
                    cat.threshold = q16_from_int(1) / 2; // 0.5 threshold
                }
            }
            SafetyMode::Custom => {
                // Leave as-is — user configures individually
            }
        }
    }

    /// Get the current safety mode
    fn get_mode(&self) -> SafetyMode {
        self.mode
    }

    // ── Category Configuration ───────────────────────────────────────

    /// Enable or disable a specific content category
    fn set_category_enabled(&mut self, cat: ContentCategory, enabled: bool) {
        let idx = category_index(cat);
        if idx < self.categories.len() {
            self.categories[idx].enabled = enabled;
            // Switch to custom mode if user is tweaking individual categories
            if self.mode != SafetyMode::Uncensored {
                self.mode = SafetyMode::Custom;
            }
        }
    }

    /// Set the threshold for a content category
    fn set_category_threshold(&mut self, cat: ContentCategory, threshold: Q16) {
        let idx = category_index(cat);
        if idx < self.categories.len() {
            self.categories[idx].threshold = threshold;
        }
    }

    /// Set the action for a content category
    fn set_category_action(&mut self, cat: ContentCategory, action: FilterAction) {
        let idx = category_index(cat);
        if idx < self.categories.len() {
            self.categories[idx].action = action;
        }
    }

    // ── User Rules ───────────────────────────────────────────────────

    /// Add a user-defined rule (allowlist or blocklist pattern)
    fn add_rule(&mut self, pattern: u64, action: FilterAction, is_allowlist: bool) -> u32 {
        if self.user_rules.len() >= MAX_USER_RULES {
            return 0;
        }

        let id = self.next_rule_id;
        self.next_rule_id = self.next_rule_id.saturating_add(1);

        self.user_rules.push(UserRule {
            id,
            pattern_hash: pattern,
            action,
            is_allowlist,
            enabled: true,
            match_count: 0,
        });

        id
    }

    /// Remove a user rule by ID
    fn remove_rule(&mut self, rule_id: u32) {
        self.user_rules.retain(|r| r.id != rule_id);
    }

    /// Enable or disable a user rule
    fn set_rule_enabled(&mut self, rule_id: u32, enabled: bool) {
        if let Some(rule) = self.user_rules.iter_mut().find(|r| r.id == rule_id) {
            rule.enabled = enabled;
        }
    }

    // ── Allowlist / Blocklist ────────────────────────────────────────

    /// Add a content hash to the allowlist (always allowed)
    fn add_to_allowlist(&mut self, content_hash: u64) {
        if self.allowlist.len() < MAX_LIST_ENTRIES && !self.allowlist.contains(&content_hash) {
            self.allowlist.push(content_hash);
        }
    }

    /// Add a content hash to the blocklist (always blocked in non-uncensored mode)
    fn add_to_blocklist(&mut self, content_hash: u64) {
        if self.blocklist.len() < MAX_LIST_ENTRIES && !self.blocklist.contains(&content_hash) {
            self.blocklist.push(content_hash);
        }
    }

    /// Remove from allowlist
    fn remove_from_allowlist(&mut self, content_hash: u64) {
        self.allowlist.retain(|&h| h != content_hash);
    }

    /// Remove from blocklist
    fn remove_from_blocklist(&mut self, content_hash: u64) {
        self.blocklist.retain(|&h| h != content_hash);
    }

    // ── Content Evaluation ───────────────────────────────────────────

    /// Evaluate content against all active filters
    /// Returns the evaluation result (allow/warn/block and why)
    fn evaluate(&mut self, content_hash: u64, category_scores: &[Q16], timestamp: u64) -> EvalResult {
        self.total_evaluations = self.total_evaluations.saturating_add(1);

        // Uncensored mode — always allow
        if self.mode == SafetyMode::Uncensored {
            self.total_allowed = self.total_allowed.saturating_add(1);
            return EvalResult {
                allowed: true,
                action: FilterAction::Allow,
                category: ContentCategory::Violence, // placeholder
                score: 0,
                rule_matched: 0,
            };
        }

        // Check allowlist first — always allow
        if self.allowlist.contains(&content_hash) {
            self.total_allowed = self.total_allowed.saturating_add(1);
            return EvalResult {
                allowed: true,
                action: FilterAction::Allow,
                category: ContentCategory::Violence,
                score: 0,
                rule_matched: 0,
            };
        }

        // Check blocklist — block unless uncensored
        if self.blocklist.contains(&content_hash) {
            self.total_blocked = self.total_blocked.saturating_add(1);
            self.log_audit(timestamp, content_hash, ContentCategory::Violence,
                          q16_from_int(1), FilterAction::Block, 0, false);
            return EvalResult {
                allowed: false,
                action: FilterAction::Block,
                category: ContentCategory::Violence,
                score: q16_from_int(1),
                rule_matched: 0,
            };
        }

        // Check user rules
        for rule in &mut self.user_rules {
            if !rule.enabled { continue; }
            // Simple pattern match via hash equality
            if content_hash == rule.pattern_hash || (content_hash ^ rule.pattern_hash) < 0x100 {
                rule.match_count += 1;
                if rule.is_allowlist {
                    self.total_allowed = self.total_allowed.saturating_add(1);
                    return EvalResult {
                        allowed: true,
                        action: FilterAction::Allow,
                        category: ContentCategory::Violence,
                        score: 0,
                        rule_matched: rule.id,
                    };
                } else {
                    let action = rule.action;
                    match action {
                        FilterAction::Block => { self.total_blocked = self.total_blocked.saturating_add(1); },
                        FilterAction::Warn => { self.total_warned = self.total_warned.saturating_add(1); },
                        _ => { self.total_allowed = self.total_allowed.saturating_add(1); },
                    }
                    self.log_audit(timestamp, content_hash, ContentCategory::Violence,
                                  q16_from_int(1), action, rule.id, false);
                    return EvalResult {
                        allowed: action != FilterAction::Block,
                        action,
                        category: ContentCategory::Violence,
                        score: q16_from_int(1),
                        rule_matched: rule.id,
                    };
                }
            }
        }

        // Check category scores against thresholds
        let mut worst_action = FilterAction::Allow;
        let mut worst_category = ContentCategory::Violence;
        let mut worst_score: Q16 = 0;

        for (idx, &score) in category_scores.iter().enumerate() {
            if idx >= self.categories.len() { break; }
            let cat_config = &self.categories[idx];

            if !cat_config.enabled { continue; }

            if score >= cat_config.threshold {
                let action = cat_config.action;
                if action_severity(action) > action_severity(worst_action) {
                    worst_action = action;
                    worst_category = cat_config.category;
                    worst_score = score;
                }
            }
        }

        // Update category triggered counts
        if worst_action != FilterAction::Allow {
            let idx = category_index(worst_category);
            if idx < self.categories.len() {
                self.categories[idx].triggered_count = self.categories[idx].triggered_count.saturating_add(1);
            }
        }

        match worst_action {
            FilterAction::Block => { self.total_blocked = self.total_blocked.saturating_add(1); },
            FilterAction::Warn => { self.total_warned = self.total_warned.saturating_add(1); },
            _ => { self.total_allowed = self.total_allowed.saturating_add(1); },
        }

        // Log the decision
        self.log_audit(timestamp, content_hash, worst_category,
                      worst_score, worst_action, 0, false);

        EvalResult {
            allowed: worst_action != FilterAction::Block,
            action: worst_action,
            category: worst_category,
            score: worst_score,
            rule_matched: 0,
        }
    }

    // ── Audit Logging ────────────────────────────────────────────────

    /// Log a safety decision to the audit trail
    fn log_audit(&mut self, timestamp: u64, content: u64, category: ContentCategory,
                  score: Q16, action: FilterAction, rule: u32, overridden: bool) {
        if self.audit_log.len() >= MAX_AUDIT_LOG {
            // Remove oldest half
            let keep = MAX_AUDIT_LOG / 2;
            let drain_count = self.audit_log.len() - keep;
            for _ in 0..drain_count {
                self.audit_log.remove(0);
            }
        }

        self.audit_log.push(AuditEntry {
            timestamp,
            content_hash: content,
            category,
            score,
            action_taken: action,
            rule_id: rule,
            overridden,
        });
    }

    /// Get recent audit entries (up to limit)
    fn get_audit_log(&self, limit: usize) -> Vec<&AuditEntry> {
        let start = if self.audit_log.len() > limit {
            self.audit_log.len() - limit
        } else {
            0
        };
        self.audit_log[start..].iter().collect()
    }

    // ── Statistics ───────────────────────────────────────────────────

    /// Get safety engine statistics
    fn get_stats(&self) -> (u64, u64, u64, u64, SafetyMode) {
        (
            self.total_evaluations,
            self.total_allowed,
            self.total_warned,
            self.total_blocked,
            self.mode,
        )
    }

    /// Get the allowance rate as Q16 (0.0 to 1.0)
    fn allowance_rate(&self) -> Q16 {
        if self.total_evaluations == 0 {
            return q16_from_int(1); // 100% if nothing evaluated
        }
        (((self.total_allowed as i64) << 16) / (self.total_evaluations as i64).max(1)) as Q16
    }
}

/// Helper: severity ranking of filter actions (higher = more severe)
fn action_severity(action: FilterAction) -> u8 {
    match action {
        FilterAction::Allow => 0,
        FilterAction::LogOnly => 1,
        FilterAction::Warn => 2,
        FilterAction::Block => 3,
    }
}

// ── Global State ─────────────────────────────────────────────────────

static ENGINE: Mutex<Option<SafetyEngine>> = Mutex::new(None);

/// Access the global safety engine
pub fn with_engine<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut SafetyEngine) -> R,
{
    let mut locked = ENGINE.lock();
    if let Some(ref mut engine) = *locked {
        Some(f(engine))
    } else {
        None
    }
}

// ── Module Initialization ────────────────────────────────────────────

pub fn init() {
    let engine = SafetyEngine::new();
    let mode = engine.mode;

    let mut locked = ENGINE.lock();
    *locked = Some(engine);

    let mode_str = match mode {
        SafetyMode::Uncensored => "UNCENSORED",
        SafetyMode::Minimal => "minimal",
        SafetyMode::Standard => "standard",
        SafetyMode::Strict => "strict",
        SafetyMode::Custom => "custom",
    };
    serial_println!("    Safety: mode={}, user-controlled filtering, guardrails, audit log ready", mode_str);
}

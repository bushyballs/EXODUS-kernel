use crate::sync::Mutex;
/// Persistent Learning Engine — the AI learns from every interaction
///
/// This module gives the Hoags AI a long-term memory of user
/// preferences, learned facts, and prompt performance. Over time,
/// the system adapts to the user's communication style, interests,
/// and expectations without any cloud dependency.
///
///   - Interaction history with feedback scoring
///   - User preference tracking (EMA-based smoothing)
///   - Fact learning with confidence decay over time
///   - Prompt configuration performance tracking
///   - Automatic memory decay for stale knowledge
///   - Context injection for personalized responses
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::{q16_from_int, q16_mul, Q16};

// ---------------------------------------------------------------------------
// Constants (Q16 fixed-point)
// ---------------------------------------------------------------------------

/// EMA decay factor: 0.9 in Q16 = 0.9 * 65536 = 58982
const EMA_OLD_WEIGHT: Q16 = 58982;

/// EMA new-value factor: 0.1 in Q16 = 0.1 * 65536 = 6554
const EMA_NEW_WEIGHT: Q16 = 6554;

/// Confidence decay per time unit: 0.999 in Q16 = 0.999 * 65536 = 65470
const CONFIDENCE_DECAY: Q16 = 65470;

/// Minimum confidence before a fact is forgotten: 0.05 in Q16
const MIN_CONFIDENCE: Q16 = 3277;

/// Default initial confidence for new facts: 0.5 in Q16 = 32768
const DEFAULT_CONFIDENCE: Q16 = 32768;

/// Time threshold for decay consideration (arbitrary units)
const DECAY_TIME_THRESHOLD: u64 = 1000;

/// Maximum number of context hashes returned
const MAX_CONTEXT_HASHES: usize = 32;

/// Maximum facts stored before pruning
const MAX_FACTS: usize = 4096;

/// Maximum preferences tracked
const MAX_PREFERENCES: usize = 64;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A record of a single user interaction with the system
#[derive(Clone, Copy)]
pub struct InteractionRecord {
    pub prompt_hash: u64,
    pub response_hash: u64,
    pub feedback_score: Q16,
    pub timestamp: u64,
    pub topic_hash: u64,
}

/// Categories of user preferences the system can learn
#[derive(Clone, Copy, PartialEq)]
pub enum PreferenceCategory {
    /// How the user prefers communication (formal, casual, etc.)
    CommunicationStyle,
    /// How much detail the user wants
    Verbosity,
    /// Interest level in various topics
    TopicInterest,
    /// User's technical skill level
    SkillLevel,
    /// Preferred output format (lists, prose, code, etc.)
    ResponseFormat,
    /// When the user typically interacts
    TimePattern,
}

/// A learned preference about the user
#[derive(Clone, Copy)]
pub struct UserPreference {
    pub category: PreferenceCategory,
    pub value: Q16,
    pub confidence: Q16,
    pub update_count: u32,
}

/// A fact the system has learned over time
#[derive(Clone, Copy)]
pub struct LearnedFact {
    pub fact_hash: u64,
    pub source_hash: u64,
    pub confidence: Q16,
    pub last_confirmed: u64,
    pub times_referenced: u32,
}

/// Performance record for a prompt configuration
#[derive(Clone, Copy)]
pub struct PromptPerformance {
    pub config_hash: u64,
    pub avg_feedback: Q16,
    pub sample_count: u32,
}

/// Comprehensive user profile built from interactions
pub struct UserProfile {
    pub preferences: Vec<UserPreference>,
    pub facts: Vec<LearnedFact>,
    pub interaction_count: u64,
    pub first_seen: u64,
    pub last_seen: u64,
    pub avg_satisfaction: Q16,
}

impl UserProfile {
    fn new() -> Self {
        UserProfile {
            preferences: Vec::new(),
            facts: Vec::new(),
            interaction_count: 0,
            first_seen: 0,
            last_seen: 0,
            avg_satisfaction: q16_from_int(0),
        }
    }
}

/// The core learning engine — maintains all learning state
pub struct LearningEngine {
    pub profile: UserProfile,
    pub interactions: Vec<InteractionRecord>,
    pub prompt_perf: Vec<PromptPerformance>,
    pub max_interactions: u32,
    pub context_summary_hash: u64,
    pub total_facts_learned: u64,
    pub total_preferences_updated: u64,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static LEARNING_ENGINE: Mutex<Option<LearningEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// LearningEngine implementation
// ---------------------------------------------------------------------------

impl LearningEngine {
    /// Create a new empty learning engine
    pub fn new() -> Self {
        LearningEngine {
            profile: UserProfile::new(),
            interactions: Vec::new(),
            prompt_perf: Vec::new(),
            max_interactions: 10000,
            context_summary_hash: 0,
            total_facts_learned: 0,
            total_preferences_updated: 0,
        }
    }

    /// Record a user interaction with feedback
    ///
    /// Stores the interaction and updates the running average
    /// satisfaction score. Old interactions are evicted when
    /// the buffer exceeds max_interactions.
    pub fn record_interaction(
        &mut self,
        prompt: u64,
        response: u64,
        feedback: Q16,
        timestamp: u64,
        topic: u64,
    ) {
        let record = InteractionRecord {
            prompt_hash: prompt,
            response_hash: response,
            feedback_score: feedback,
            timestamp,
            topic_hash: topic,
        };

        self.interactions.push(record);

        // Evict oldest interactions if over capacity
        if self.interactions.len() > self.max_interactions as usize {
            let excess = self.interactions.len() - self.max_interactions as usize;
            // Remove the oldest entries by draining from the front
            for _ in 0..excess {
                self.interactions.remove(0);
            }
        }

        // Update profile stats
        self.profile.interaction_count = self.profile.interaction_count.saturating_add(1);
        if self.profile.first_seen == 0 {
            self.profile.first_seen = timestamp;
        }
        self.profile.last_seen = timestamp;

        // Update rolling average satisfaction using EMA
        self.profile.avg_satisfaction = q16_mul(self.profile.avg_satisfaction, EMA_OLD_WEIGHT)
            + q16_mul(feedback, EMA_NEW_WEIGHT);

        // Update context summary hash (simple XOR accumulation)
        self.context_summary_hash ^= prompt ^ topic;
    }

    /// Update a user preference using exponential moving average
    ///
    /// The EMA formula smoothly blends old and new values:
    ///   new_pref = old_pref * 0.9 + observed_value * 0.1
    ///
    /// This prevents sudden shifts while still adapting over time.
    pub fn update_preference(&mut self, cat: PreferenceCategory, value: Q16) {
        // Search for existing preference in this category
        let mut found = false;
        for pref in self.profile.preferences.iter_mut() {
            if pref.category == cat {
                // EMA update: old * 0.9 + new * 0.1
                pref.value = q16_mul(pref.value, EMA_OLD_WEIGHT) + q16_mul(value, EMA_NEW_WEIGHT);

                // Confidence increases with more observations (capped at 1.0)
                let confidence_bump: Q16 = 655; // ~0.01 in Q16
                let max_confidence: Q16 = q16_from_int(1);
                pref.confidence += confidence_bump;
                if pref.confidence > max_confidence {
                    pref.confidence = max_confidence;
                }

                pref.update_count += 1;
                found = true;
                break;
            }
        }

        if !found {
            // Create new preference entry
            if self.profile.preferences.len() < MAX_PREFERENCES {
                self.profile.preferences.push(UserPreference {
                    category: cat,
                    value,
                    confidence: DEFAULT_CONFIDENCE,
                    update_count: 1,
                });
            }
        }

        self.total_preferences_updated = self.total_preferences_updated.saturating_add(1);
    }

    /// Learn a new fact or reinforce an existing one
    ///
    /// If the fact already exists, its confidence is boosted
    /// and the reference count incremented. Otherwise a new
    /// fact entry is created.
    pub fn learn_fact(&mut self, fact: u64, source: u64, confidence: Q16, timestamp: u64) {
        // Check if we already know this fact
        for existing in self.profile.facts.iter_mut() {
            if existing.fact_hash == fact {
                // Reinforce: blend confidences, prefer higher
                let blended = q16_mul(existing.confidence, EMA_OLD_WEIGHT)
                    + q16_mul(confidence, EMA_NEW_WEIGHT);
                // Take the max of blended and new confidence
                if blended > existing.confidence {
                    existing.confidence = blended;
                }
                existing.last_confirmed = timestamp;
                existing.times_referenced += 1;
                return;
            }
        }

        // Prune low-confidence facts if at capacity
        if self.profile.facts.len() >= MAX_FACTS {
            self.prune_weakest_facts();
        }

        // Store new fact
        self.profile.facts.push(LearnedFact {
            fact_hash: fact,
            source_hash: source,
            confidence,
            last_confirmed: timestamp,
            times_referenced: 1,
        });

        self.total_facts_learned = self.total_facts_learned.saturating_add(1);
    }

    /// Forget a specific fact by hash
    pub fn forget_fact(&mut self, fact: u64) {
        self.profile.facts.retain(|f| f.fact_hash != fact);
    }

    /// Get context hashes for injection into prompts
    ///
    /// Returns the most relevant fact and preference hashes
    /// sorted by confidence, for the LLM context window.
    pub fn get_user_context(&self) -> Vec<u64> {
        let mut context: Vec<u64> = Vec::new();

        // Add high-confidence facts first (sorted by confidence desc)
        let mut fact_indices: Vec<usize> = (0..self.profile.facts.len()).collect();
        // Simple selection sort by confidence (descending)
        for i in 0..fact_indices.len() {
            let mut best = i;
            for j in (i + 1)..fact_indices.len() {
                if self.profile.facts[fact_indices[j]].confidence
                    > self.profile.facts[fact_indices[best]].confidence
                {
                    best = j;
                }
            }
            if best != i {
                fact_indices.swap(i, best);
            }
        }

        for &idx in fact_indices.iter() {
            if context.len() >= MAX_CONTEXT_HASHES {
                break;
            }
            let fact = &self.profile.facts[idx];
            if fact.confidence > MIN_CONFIDENCE {
                context.push(fact.fact_hash);
            }
        }

        // Add preference hashes (category as u64 XOR'd with value)
        for pref in self.profile.preferences.iter() {
            if context.len() >= MAX_CONTEXT_HASHES {
                break;
            }
            if pref.confidence > DEFAULT_CONFIDENCE {
                let pref_hash = (pref.category as u64) ^ (pref.value as u64);
                context.push(pref_hash);
            }
        }

        // Always include context summary
        if context.len() < MAX_CONTEXT_HASHES && self.context_summary_hash != 0 {
            context.push(self.context_summary_hash);
        }

        context
    }

    /// Get the best-performing prompt configuration hash
    ///
    /// Returns the config_hash with the highest average feedback
    /// score, provided it has enough samples for statistical
    /// significance (at least 5 samples).
    pub fn get_prompt_recommendation(&self) -> u64 {
        let min_samples: u32 = 5;
        let mut best_hash: u64 = 0;
        let mut best_score: Q16 = i32::MIN;

        for perf in self.prompt_perf.iter() {
            if perf.sample_count >= min_samples && perf.avg_feedback > best_score {
                best_score = perf.avg_feedback;
                best_hash = perf.config_hash;
            }
        }

        // If no config has enough samples, return the one with most data
        if best_hash == 0 {
            let mut most_samples: u32 = 0;
            for perf in self.prompt_perf.iter() {
                if perf.sample_count > most_samples {
                    most_samples = perf.sample_count;
                    best_hash = perf.config_hash;
                }
            }
        }

        best_hash
    }

    /// Record performance feedback for a prompt configuration
    ///
    /// Updates the running average feedback score for the given
    /// config hash. Creates a new entry if not seen before.
    pub fn record_prompt_performance(&mut self, config: u64, feedback: Q16) {
        for perf in self.prompt_perf.iter_mut() {
            if perf.config_hash == config {
                // Incremental mean: avg = avg + (new - avg) / n
                perf.sample_count += 1;
                let n = q16_from_int(perf.sample_count as i32);
                let _diff = feedback - perf.avg_feedback;
                // Division by n approximated: diff * (1/n)
                // For Q16: (diff << 16) / n, but we use q16_mul with reciprocal
                // Simpler: use EMA for stability
                perf.avg_feedback =
                    q16_mul(perf.avg_feedback, EMA_OLD_WEIGHT) + q16_mul(feedback, EMA_NEW_WEIGHT);
                let _ = n; // used for count, EMA doesn't need exact division
                return;
            }
        }

        // New config entry
        self.prompt_perf.push(PromptPerformance {
            config_hash: config,
            avg_feedback: feedback,
            sample_count: 1,
        });
    }

    /// Decay confidence of old facts
    ///
    /// Facts that haven't been confirmed recently have their
    /// confidence reduced. Facts below the minimum threshold
    /// are automatically forgotten.
    pub fn decay_old_memories(&mut self, current_time: u64) {
        let mut to_remove: Vec<usize> = Vec::new();

        for (i, fact) in self.profile.facts.iter_mut().enumerate() {
            let age = current_time.saturating_sub(fact.last_confirmed);

            if age > DECAY_TIME_THRESHOLD {
                // Apply decay proportional to how many threshold periods have passed
                let decay_periods = age / DECAY_TIME_THRESHOLD;

                // Apply decay iteratively (compound decay)
                let mut decayed_confidence = fact.confidence;
                let mut periods = decay_periods;
                // Cap iterations to prevent excessive spinning
                if periods > 100 {
                    periods = 100;
                }
                for _ in 0..periods {
                    decayed_confidence = q16_mul(decayed_confidence, CONFIDENCE_DECAY);
                }
                fact.confidence = decayed_confidence;

                // Mark for removal if below threshold
                if fact.confidence < MIN_CONFIDENCE {
                    to_remove.push(i);
                }
            }
        }

        // Remove dead facts in reverse order to preserve indices
        let mut removed = 0;
        for idx in to_remove.iter() {
            let adjusted = idx - removed;
            if adjusted < self.profile.facts.len() {
                self.profile.facts.remove(adjusted);
                removed += 1;
            }
        }
    }

    /// Get learning statistics
    ///
    /// Returns: (total_interactions, total_facts, preference_count, avg_satisfaction)
    pub fn get_learning_stats(&self) -> (u64, u64, u32, Q16) {
        (
            self.profile.interaction_count,
            self.total_facts_learned,
            self.profile.preferences.len() as u32,
            self.profile.avg_satisfaction,
        )
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Remove the lowest-confidence facts to make room for new ones
    fn prune_weakest_facts(&mut self) {
        if self.profile.facts.is_empty() {
            return;
        }

        // Find the minimum confidence
        let mut min_confidence: Q16 = i32::MAX;
        for fact in self.profile.facts.iter() {
            if fact.confidence < min_confidence {
                min_confidence = fact.confidence;
            }
        }

        // Remove all facts at minimum confidence (free a batch)
        self.profile.facts.retain(|f| f.confidence > min_confidence);
    }

    /// Compute a simple topic frequency distribution
    ///
    /// Returns up to `limit` (topic_hash, count) pairs sorted by frequency
    pub fn top_topics(&self, limit: usize) -> Vec<(u64, u32)> {
        // Accumulate topic counts using a simple vec-based map
        let mut topic_counts: Vec<(u64, u32)> = Vec::new();

        for interaction in self.interactions.iter() {
            let topic = interaction.topic_hash;
            let mut found = false;
            for entry in topic_counts.iter_mut() {
                if entry.0 == topic {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                topic_counts.push((topic, 1));
            }
        }

        // Sort descending by count (selection sort)
        for i in 0..topic_counts.len() {
            let mut best = i;
            for j in (i + 1)..topic_counts.len() {
                if topic_counts[j].1 > topic_counts[best].1 {
                    best = j;
                }
            }
            if best != i {
                topic_counts.swap(i, best);
            }
        }

        // Truncate to limit
        if topic_counts.len() > limit {
            topic_counts.truncate(limit);
        }

        topic_counts
    }

    /// Get the preference value for a given category, if known
    pub fn get_preference(&self, cat: PreferenceCategory) -> Option<Q16> {
        for pref in self.profile.preferences.iter() {
            if pref.category == cat {
                return Some(pref.value);
            }
        }
        None
    }

    /// Get total time span the user has been interacting (last - first seen)
    pub fn user_tenure(&self) -> u64 {
        self.profile
            .last_seen
            .saturating_sub(self.profile.first_seen)
    }
}

// ---------------------------------------------------------------------------
// Module initialization
// ---------------------------------------------------------------------------

/// Initialize the persistent learning engine
///
/// Creates a fresh LearningEngine and stores it in the global mutex.
/// Called during LLM subsystem startup.
pub fn init() {
    let engine = LearningEngine::new();
    let mut guard = LEARNING_ENGINE.lock();
    *guard = Some(engine);
    serial_println!("  Learning engine initialized — persistent user adaptation active");
}

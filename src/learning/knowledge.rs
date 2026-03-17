use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
/// Persistent knowledge store for Genesis learning subsystem
///
/// Stores and retrieves learned facts, preferences, and contextual memory:
///   - Facts: verified truths about the user and their environment
///   - Preferences: weighted user preferences across categories
///   - Contextual memory: situation-specific knowledge with decay
///   - Associative recall: connect related pieces of knowledge
///   - Knowledge consolidation: merge and strengthen related entries
///   - Forgetting curve: naturally decay unused knowledge
///
/// All math is Q16 fixed-point (i32, 16 fractional bits).
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// ── Q16 fixed-point ────────────────────────────────────────────────────────

const Q16_ONE: i32 = 65536;
const Q16_HALF: i32 = 32768;
const Q16_ZERO: i32 = 0;
const Q16_QUARTER: i32 = 16384;
const Q16_TENTH: i32 = 6554;
const Q16_HUNDREDTH: i32 = 655;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

fn q16_clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ── Configuration ──────────────────────────────────────────────────────────

const MAX_FACTS: usize = 512;
const MAX_PREFERENCES: usize = 256;
const MAX_CONTEXT_MEMORIES: usize = 256;
const MAX_ASSOCIATIONS: usize = 1024;
const MAX_TAGS_PER_ENTRY: usize = 8;
const ASSOCIATION_THRESHOLD: i32 = 9830; // 0.15 Q16 — minimum strength for an association

// ── Types ──────────────────────────────────────────────────────────────────

/// Category of a known fact
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactCategory {
    UserIdentity, // name, timezone, language
    SystemConfig, // preferred settings, hardware info
    AppBehavior,  // how apps are typically configured/used
    Environment,  // network, location, connected devices
    Social,       // contacts, communication patterns
    Temporal,     // schedule, routines, deadlines
    ContentPref,  // preferred content types, topics
    Workflow,     // task patterns, project structures
}

/// Confidence level in a fact
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Certainty {
    Speculative, // inferred, low confidence
    Probable,    // multiple signals, moderate confidence
    Confident,   // strong evidence
    Verified,    // explicitly confirmed by user
}

impl Certainty {
    fn to_q16(self) -> i32 {
        match self {
            Certainty::Speculative => Q16_QUARTER,
            Certainty::Probable => Q16_HALF,
            Certainty::Confident => 49152, // 0.75
            Certainty::Verified => Q16_ONE,
        }
    }

    fn from_q16(v: i32) -> Self {
        if v >= 49152 {
            Certainty::Verified
        } else if v >= Q16_HALF {
            Certainty::Confident
        } else if v >= Q16_QUARTER {
            Certainty::Probable
        } else {
            Certainty::Speculative
        }
    }
}

/// A known fact about the user or environment
pub struct Fact {
    pub id: u32,
    pub category: FactCategory,
    pub key: String,
    pub value: String,
    pub certainty: Certainty,
    pub confidence: i32, // Q16 [0..1] continuous confidence
    pub access_count: u32,
    pub reinforcement_count: u32, // times this fact was re-observed
    pub created_at: u64,
    pub last_accessed: u64,
    pub last_reinforced: u64,
    pub tags: Vec<u32>,             // tag hashes for associative lookup
    pub source_hash: u32,           // hash of the source that provided this fact
    pub superseded_by: Option<u32>, // if updated, points to newer fact
}

/// A user preference (key -> weighted value)
pub struct Preference {
    pub id: u32,
    pub category: FactCategory,
    pub key: String,
    pub value: String,
    pub weight: i32,    // Q16 [0..1] how strongly preferred
    pub stability: i32, // Q16 [0..1] how consistent over time
    pub observation_count: u32,
    pub last_observed: u64,
    pub contradictions: u32, // times opposite preference was observed
}

/// A contextual memory: knowledge tied to a specific situation
pub struct ContextMemory {
    pub id: u32,
    pub context_hash: u32, // hash identifying the context
    pub content_key: String,
    pub content_value: String,
    pub relevance: i32, // Q16 [0..1] how relevant in this context
    pub retention: i32, // Q16 [0..1] how well-retained (decays over time)
    pub access_count: u32,
    pub created_at: u64,
    pub last_accessed: u64,
    pub expiry: u64, // timestamp after which this memory expires (0 = never)
}

/// An association between two knowledge entries
pub struct Association {
    pub from_id: u32,
    pub to_id: u32,
    pub strength: i32,        // Q16 [0..1] association strength
    pub co_access_count: u32, // times both were accessed together
    pub last_co_access: u64,
}

/// Simple hash for strings (djb2 algorithm)
fn hash_str(s: &str) -> u32 {
    let mut hash: u32 = 5381;
    for byte in s.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(*byte as u32);
    }
    hash
}

/// The persistent knowledge store
pub struct KnowledgeStore {
    pub enabled: bool,
    pub facts: Vec<Fact>,
    pub preferences: Vec<Preference>,
    pub context_memories: Vec<ContextMemory>,
    pub associations: Vec<Association>,
    pub next_id: u32,

    // Forgetting curve parameters
    pub base_decay_rate: i32, // Q16 per-cycle decay for unaccessed items
    pub access_boost: i32,    // Q16 retention boost per access
    pub consolidation_threshold: i32, // Q16 minimum strength to consolidate

    // Stats
    pub total_recalls: u64,
    pub successful_recalls: u64,
}

impl KnowledgeStore {
    const fn new() -> Self {
        KnowledgeStore {
            enabled: true,
            facts: Vec::new(),
            preferences: Vec::new(),
            context_memories: Vec::new(),
            associations: Vec::new(),
            next_id: 1,
            base_decay_rate: 64880,  // 0.99 per cycle
            access_boost: Q16_TENTH, // 0.1 boost per access
            consolidation_threshold: Q16_HALF,
            total_recalls: 0,
            successful_recalls: 0,
        }
    }

    fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    // ── Facts ──────────────────────────────────────────────────────────────

    /// Store a new fact or reinforce an existing one
    pub fn learn_fact(
        &mut self,
        category: FactCategory,
        key: &str,
        value: &str,
        certainty: Certainty,
        timestamp: u64,
    ) -> u32 {
        if !self.enabled {
            return 0;
        }

        // Check for existing fact with same key
        for fact in self.facts.iter_mut() {
            if fact.key.as_str() == key && fact.superseded_by.is_none() {
                if fact.value.as_str() == value {
                    // Same fact: reinforce
                    fact.reinforcement_count = fact.reinforcement_count.saturating_add(1);
                    fact.last_reinforced = timestamp;
                    // Boost confidence
                    fact.confidence = q16_clamp(fact.confidence + Q16_TENTH, Q16_ZERO, Q16_ONE);
                    // Upgrade certainty if new evidence is stronger
                    if certainty > fact.certainty {
                        fact.certainty = certainty;
                    }
                    return fact.id;
                } else {
                    // Different value: supersede old fact
                    // Allocate a new id without re-borrowing the facts slice
                    let new_id = self.next_id;
                    self.next_id = self.next_id.saturating_add(1);
                    fact.superseded_by = Some(new_id);
                    // Reduce confidence in old fact
                    fact.confidence = q16_mul(fact.confidence, Q16_HALF);

                    if self.facts.len() < MAX_FACTS {
                        let tag = hash_str(key);
                        self.facts.push(Fact {
                            id: new_id,
                            category,
                            key: String::from(key),
                            value: String::from(value),
                            certainty,
                            confidence: certainty.to_q16(),
                            access_count: 0,
                            reinforcement_count: 0,
                            created_at: timestamp,
                            last_accessed: timestamp,
                            last_reinforced: timestamp,
                            tags: vec![tag],
                            source_hash: 0,
                            superseded_by: None,
                        });
                    }
                    return self.next_id - 1;
                }
            }
        }

        // Brand new fact
        let id = self.alloc_id();
        if self.facts.len() < MAX_FACTS {
            let tag = hash_str(key);
            self.facts.push(Fact {
                id,
                category,
                key: String::from(key),
                value: String::from(value),
                certainty,
                confidence: certainty.to_q16(),
                access_count: 0,
                reinforcement_count: 0,
                created_at: timestamp,
                last_accessed: timestamp,
                last_reinforced: timestamp,
                tags: vec![tag],
                source_hash: 0,
                superseded_by: None,
            });
        }
        id
    }

    /// Recall a fact by key, updating access stats
    pub fn recall_fact(&mut self, key: &str, timestamp: u64) -> Option<(&str, Certainty, i32)> {
        self.total_recalls = self.total_recalls.saturating_add(1);

        for fact in self.facts.iter_mut() {
            if fact.key.as_str() == key && fact.superseded_by.is_none() {
                fact.access_count = fact.access_count.saturating_add(1);
                fact.last_accessed = timestamp;
                // Boost retention
                fact.confidence = q16_clamp(
                    fact.confidence + q16_mul(self.access_boost, Q16_HALF),
                    Q16_ZERO,
                    Q16_ONE,
                );
                self.successful_recalls = self.successful_recalls.saturating_add(1);
                return Some((&fact.value, fact.certainty, fact.confidence));
            }
        }
        None
    }

    /// Query facts by category
    pub fn facts_by_category(&self, category: FactCategory) -> Vec<(u32, &str, &str, i32)> {
        let mut results = Vec::new();
        for fact in &self.facts {
            if fact.category == category && fact.superseded_by.is_none() {
                results.push((
                    fact.id,
                    fact.key.as_str(),
                    fact.value.as_str(),
                    fact.confidence,
                ));
            }
        }
        results
    }

    // ── Preferences ────────────────────────────────────────────────────────

    /// Record an observed preference
    pub fn observe_preference(
        &mut self,
        category: FactCategory,
        key: &str,
        value: &str,
        timestamp: u64,
    ) -> u32 {
        if !self.enabled {
            return 0;
        }

        // Check for existing preference
        for pref in self.preferences.iter_mut() {
            if pref.key.as_str() == key {
                if pref.value.as_str() == value {
                    // Same preference: reinforce
                    pref.observation_count = pref.observation_count.saturating_add(1);
                    pref.last_observed = timestamp;
                    pref.weight = q16_clamp(pref.weight + Q16_HUNDREDTH, Q16_ZERO, Q16_ONE);
                    // Stability: grows with consistent observations
                    let consistency = q16_div(
                        pref.observation_count as i32,
                        (pref.observation_count + pref.contradictions) as i32,
                    );
                    pref.stability = q16_mul(pref.stability, 52429) // 0.8
                        + q16_mul(consistency, 13107); // 0.2
                    return pref.id;
                } else {
                    // Different value: contradiction
                    pref.contradictions = pref.contradictions.saturating_add(1);
                    pref.stability = q16_mul(pref.stability, 52429); // weaken stability
                                                                     // If contradictions dominate, replace
                    if pref.contradictions > pref.observation_count {
                        pref.value = String::from(value);
                        pref.observation_count = 1;
                        pref.contradictions = 0;
                        pref.weight = Q16_TENTH;
                        pref.stability = Q16_TENTH;
                    }
                    return pref.id;
                }
            }
        }

        // New preference
        let id = self.alloc_id();
        if self.preferences.len() < MAX_PREFERENCES {
            self.preferences.push(Preference {
                id,
                category,
                key: String::from(key),
                value: String::from(value),
                weight: Q16_TENTH,
                stability: Q16_TENTH,
                observation_count: 1,
                last_observed: timestamp,
                contradictions: 0,
            });
        }
        id
    }

    /// Get the preferred value for a key (if confident enough)
    pub fn get_preference(&self, key: &str, min_weight: i32) -> Option<(&str, i32)> {
        for pref in &self.preferences {
            if pref.key.as_str() == key && pref.weight >= min_weight {
                return Some((&pref.value, pref.weight));
            }
        }
        None
    }

    // ── Contextual Memory ──────────────────────────────────────────────────

    /// Store a context-specific memory
    pub fn store_context_memory(
        &mut self,
        context_hash: u32,
        key: &str,
        value: &str,
        relevance: i32,
        timestamp: u64,
        expiry: u64,
    ) -> u32 {
        if !self.enabled {
            return 0;
        }

        // Check for existing context memory
        for mem in self.context_memories.iter_mut() {
            if mem.context_hash == context_hash && mem.content_key.as_str() == key {
                mem.content_value = String::from(value);
                mem.relevance = q16_clamp(relevance, Q16_ZERO, Q16_ONE);
                mem.retention = Q16_ONE; // fully refreshed
                mem.last_accessed = timestamp;
                mem.access_count = mem.access_count.saturating_add(1);
                return mem.id;
            }
        }

        // New context memory
        let id = self.alloc_id();
        if self.context_memories.len() < MAX_CONTEXT_MEMORIES {
            self.context_memories.push(ContextMemory {
                id,
                context_hash,
                content_key: String::from(key),
                content_value: String::from(value),
                relevance: q16_clamp(relevance, Q16_ZERO, Q16_ONE),
                retention: Q16_ONE,
                access_count: 1,
                created_at: timestamp,
                last_accessed: timestamp,
                expiry,
            });
        }
        id
    }

    /// Recall context-specific memories
    pub fn recall_context(
        &mut self,
        context_hash: u32,
        timestamp: u64,
    ) -> Vec<(u32, &str, &str, i32)> {
        let mut results = Vec::new();
        for mem in self.context_memories.iter_mut() {
            if mem.context_hash == context_hash && mem.retention > Q16_HUNDREDTH {
                if mem.expiry > 0 && timestamp > mem.expiry {
                    continue;
                }
                mem.access_count = mem.access_count.saturating_add(1);
                mem.last_accessed = timestamp;
                // Boost retention on access
                mem.retention = q16_clamp(
                    mem.retention + q16_mul(self.access_boost, Q16_QUARTER),
                    Q16_ZERO,
                    Q16_ONE,
                );
                results.push((
                    mem.id,
                    mem.content_key.as_str(),
                    mem.content_value.as_str(),
                    mem.relevance,
                ));
            }
        }
        results
    }

    // ── Associations ───────────────────────────────────────────────────────

    /// Create or strengthen an association between two knowledge entries
    pub fn associate(&mut self, id_a: u32, id_b: u32, timestamp: u64) {
        let (from, to) = if id_a < id_b {
            (id_a, id_b)
        } else {
            (id_b, id_a)
        };

        for assoc in self.associations.iter_mut() {
            if assoc.from_id == from && assoc.to_id == to {
                assoc.co_access_count = assoc.co_access_count.saturating_add(1);
                assoc.last_co_access = timestamp;
                // Strengthen by diminishing amounts
                let boost = q16_div(Q16_TENTH, (assoc.co_access_count as i32) + 1);
                assoc.strength = q16_clamp(assoc.strength + boost, Q16_ZERO, Q16_ONE);
                return;
            }
        }

        if self.associations.len() < MAX_ASSOCIATIONS {
            self.associations.push(Association {
                from_id: from,
                to_id: to,
                strength: Q16_TENTH,
                co_access_count: 1,
                last_co_access: timestamp,
            });
        }
    }

    /// Find all entries associated with a given ID (associative recall)
    pub fn recall_associated(&self, id: u32, min_strength: i32) -> Vec<(u32, i32)> {
        let mut results = Vec::new();
        for assoc in &self.associations {
            if assoc.strength < min_strength {
                continue;
            }
            if assoc.from_id == id {
                results.push((assoc.to_id, assoc.strength));
            } else if assoc.to_id == id {
                results.push((assoc.from_id, assoc.strength));
            }
        }

        // Sort by strength descending
        for i in 1..results.len() {
            let mut j = i;
            while j > 0 && results[j].1 > results[j - 1].1 {
                results.swap(j, j - 1);
                j -= 1;
            }
        }

        results
    }

    // ── Maintenance ────────────────────────────────────────────────────────

    /// Apply forgetting curve: decay retention and confidence of unaccessed items
    pub fn apply_forgetting(&mut self, _current_timestamp: u64) {
        let decay = self.base_decay_rate;

        // Decay facts
        for fact in self.facts.iter_mut() {
            if fact.superseded_by.is_some() {
                continue;
            }
            // Verified facts decay much slower
            let effective_decay = match fact.certainty {
                Certainty::Verified => 65209,    // 0.995
                Certainty::Confident => 64880,   // 0.99
                Certainty::Probable => decay,    // 0.99
                Certainty::Speculative => 62259, // 0.95
            };
            fact.confidence = q16_mul(fact.confidence, effective_decay);
        }

        // Decay context memories
        for mem in self.context_memories.iter_mut() {
            mem.retention = q16_mul(mem.retention, decay);
            mem.relevance = q16_mul(mem.relevance, 65209); // 0.995
        }

        // Decay associations
        for assoc in self.associations.iter_mut() {
            assoc.strength = q16_mul(assoc.strength, decay);
        }

        // Prune
        self.facts
            .retain(|f| f.confidence > Q16_HUNDREDTH || f.superseded_by.is_none());
        self.context_memories
            .retain(|m| m.retention > Q16_HUNDREDTH);
        self.associations
            .retain(|a| a.strength > ASSOCIATION_THRESHOLD);

        // Decay preference weights (slower than facts)
        for pref in self.preferences.iter_mut() {
            pref.weight = q16_mul(pref.weight, 65209); // 0.995
        }
        self.preferences.retain(|p| p.weight > Q16_HUNDREDTH);
    }

    /// Consolidate related knowledge entries
    pub fn consolidate(&mut self) {
        // Find pairs of facts with strong associations and merge confidence
        let assoc_snapshot: Vec<(u32, u32, i32)> = self
            .associations
            .iter()
            .filter(|a| a.strength >= self.consolidation_threshold)
            .map(|a| (a.from_id, a.to_id, a.strength))
            .collect();

        for (from_id, to_id, strength) in assoc_snapshot {
            // Find both facts and boost the weaker one
            let mut from_conf: Option<i32> = None;
            let mut to_conf: Option<i32> = None;

            for fact in self.facts.iter() {
                if fact.id == from_id {
                    from_conf = Some(fact.confidence);
                }
                if fact.id == to_id {
                    to_conf = Some(fact.confidence);
                }
            }

            if let (Some(fc), Some(tc)) = (from_conf, to_conf) {
                let boost = q16_mul(strength, Q16_HUNDREDTH);
                let target_id = if fc < tc { from_id } else { to_id };
                for fact in self.facts.iter_mut() {
                    if fact.id == target_id {
                        fact.confidence = q16_clamp(fact.confidence + boost, Q16_ZERO, Q16_ONE);
                        break;
                    }
                }
            }
        }
    }

    /// Get recall success rate (Q16)
    pub fn recall_rate(&self) -> i32 {
        if self.total_recalls == 0 {
            return Q16_HALF;
        }
        q16_div(self.successful_recalls as i32, self.total_recalls as i32)
    }
}

// ── Global state ───────────────────────────────────────────────────────────

static KNOWLEDGE: Mutex<Option<KnowledgeStore>> = Mutex::new(None);

pub fn init() {
    let mut guard = KNOWLEDGE.lock();
    *guard = Some(KnowledgeStore::new());
    serial_println!("    [learning] Knowledge store initialized");
}

/// Learn a new fact
pub fn learn_fact(
    category: FactCategory,
    key: &str,
    value: &str,
    certainty: Certainty,
    timestamp: u64,
) -> u32 {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store.learn_fact(category, key, value, certainty, timestamp)
    } else {
        0
    }
}

/// Recall a fact by key
pub fn recall_fact(key: &str, timestamp: u64) -> Option<(String, i32)> {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store
            .recall_fact(key, timestamp)
            .map(|(v, _cert, conf)| (String::from(v), conf))
    } else {
        None
    }
}

/// Observe a preference
pub fn observe_preference(category: FactCategory, key: &str, value: &str, timestamp: u64) -> u32 {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store.observe_preference(category, key, value, timestamp)
    } else {
        0
    }
}

/// Store a context memory
pub fn store_context(ctx: u32, key: &str, value: &str, relevance: i32, timestamp: u64) -> u32 {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store.store_context_memory(ctx, key, value, relevance, timestamp, 0)
    } else {
        0
    }
}

/// Create an association between two knowledge entries
pub fn associate(id_a: u32, id_b: u32, timestamp: u64) {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store.associate(id_a, id_b, timestamp);
    }
}

/// Run forgetting and consolidation
pub fn maintain(timestamp: u64) {
    let mut guard = KNOWLEDGE.lock();
    if let Some(store) = guard.as_mut() {
        store.apply_forgetting(timestamp);
        store.consolidate();
    }
}

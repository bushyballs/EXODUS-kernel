use crate::sync::Mutex;
use alloc::string::String;
/// Agent working memory and episodic memory
///
/// Part of the AIOS agent layer. Provides a bounded working memory
/// (sliding window) and episodic long-term memory with relevance-based
/// recall using hash similarity.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// A memory episode from a past agent interaction
#[derive(Clone)]
pub struct Episode {
    pub summary: String,
    pub summary_hash: u64,
    pub tags: Vec<u64>, // Hashed tags for fast similarity
    pub timestamp: u64,
    pub success: bool,
    pub relevance: u32,    // Cached relevance score (0-1000)
    pub access_count: u32, // Times recalled
}

/// Priority of a working memory item
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemoryPriority {
    Low,
    Normal,
    High,
    Pinned, // Never evicted by sliding window
}

/// A working memory entry
#[derive(Clone)]
pub struct WorkingItem {
    pub content: String,
    pub content_hash: u64,
    pub priority: MemoryPriority,
    pub added_at: u64,
}

struct AgentMemoryInner {
    working: Vec<WorkingItem>,
    episodes: Vec<Episode>,
    max_working: usize,
    max_episodes: usize,
    total_recalls: u64,
    total_stored: u64,
}

static MEMORY: Mutex<Option<AgentMemoryInner>> = Mutex::new(None);

/// Simple hash function for content (FNV-1a style)
fn hash_content(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Compute similarity between two hash sets (Jaccard-like)
fn tag_similarity(a: &[u64], b: &[u64]) -> u32 {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut matches = 0u32;
    for tag in a {
        if b.contains(tag) {
            matches += 1;
        }
    }
    let union = (a.len() + b.len() - matches as usize) as u32;
    if union == 0 {
        return 0;
    }
    (matches * 1000) / union
}

impl AgentMemoryInner {
    fn new(max_working: usize, max_episodes: usize) -> Self {
        AgentMemoryInner {
            working: Vec::new(),
            episodes: Vec::new(),
            max_working,
            max_episodes,
            total_recalls: 0,
            total_stored: 0,
        }
    }

    /// Push an item to working memory, evicting lowest-priority if full
    fn push_working(&mut self, content: String, priority: MemoryPriority, now: u64) {
        let content_hash = hash_content(&content);
        // Check for duplicate
        if self.working.iter().any(|w| w.content_hash == content_hash) {
            return;
        }

        // Evict if at capacity
        if self.working.len() >= self.max_working {
            // Find lowest-priority non-pinned item
            let evict_idx = self
                .working
                .iter()
                .enumerate()
                .filter(|(_, w)| w.priority != MemoryPriority::Pinned)
                .min_by_key(|(_, w)| match w.priority {
                    MemoryPriority::Low => 0,
                    MemoryPriority::Normal => 1,
                    MemoryPriority::High => 2,
                    MemoryPriority::Pinned => 3,
                })
                .map(|(i, _)| i);

            if let Some(idx) = evict_idx {
                self.working.remove(idx);
            } else {
                return; // All pinned, can't evict
            }
        }

        self.working.push(WorkingItem {
            content,
            content_hash,
            priority,
            added_at: now,
        });
    }

    /// Store an episode to long-term memory
    fn store_episode(&mut self, summary: String, tags: Vec<u64>, timestamp: u64, success: bool) {
        let summary_hash = hash_content(&summary);

        // Evict oldest if at capacity
        if self.episodes.len() >= self.max_episodes {
            // Remove least-accessed episode
            let evict_idx = self
                .episodes
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.access_count)
                .map(|(i, _)| i);
            if let Some(idx) = evict_idx {
                self.episodes.remove(idx);
            }
        }

        self.episodes.push(Episode {
            summary,
            summary_hash,
            tags,
            timestamp,
            success,
            relevance: 0,
            access_count: 0,
        });
        self.total_stored = self.total_stored.saturating_add(1);
    }

    /// Recall episodes relevant to a query (by tag similarity)
    fn recall(&mut self, query_tags: &[u64], top_k: usize) -> Vec<usize> {
        self.total_recalls = self.total_recalls.saturating_add(1);

        // Score all episodes by tag similarity
        let mut scored: Vec<(usize, u32)> = self
            .episodes
            .iter()
            .enumerate()
            .map(|(i, ep)| (i, tag_similarity(query_tags, &ep.tags)))
            .filter(|(_, score)| *score > 0)
            .collect();

        // Sort descending by score
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(top_k);

        // Update access counts and relevance
        for &(idx, score) in &scored {
            self.episodes[idx].access_count = self.episodes[idx].access_count.saturating_add(1);
            self.episodes[idx].relevance = score;
        }

        scored.iter().map(|(idx, _)| *idx).collect()
    }

    /// Get all working memory items
    fn get_working(&self) -> &[WorkingItem] {
        &self.working
    }

    /// Clear working memory (keep episodes)
    fn clear_working(&mut self) {
        self.working.clear();
    }

    /// Get an episode by index
    fn get_episode(&self, idx: usize) -> Option<&Episode> {
        self.episodes.get(idx)
    }
}

// --- Public API ---

/// Push to working memory
pub fn push_working(content: String, priority: MemoryPriority, now: u64) {
    let mut mem = MEMORY.lock();
    if let Some(m) = mem.as_mut() {
        m.push_working(content, priority, now);
    }
}

/// Store a long-term episode
pub fn store_episode(summary: String, tags: Vec<u64>, timestamp: u64, success: bool) {
    let mut mem = MEMORY.lock();
    if let Some(m) = mem.as_mut() {
        m.store_episode(summary, tags, timestamp, success);
    }
}

/// Recall relevant episodes (returns indices)
pub fn recall(query_tags: &[u64], top_k: usize) -> Vec<usize> {
    let mut mem = MEMORY.lock();
    match mem.as_mut() {
        Some(m) => m.recall(query_tags, top_k),
        None => Vec::new(),
    }
}

/// Clear working memory for new session
pub fn clear_working() {
    let mut mem = MEMORY.lock();
    if let Some(m) = mem.as_mut() {
        m.clear_working();
    }
}

/// Get working memory count
pub fn working_count() -> usize {
    let mem = MEMORY.lock();
    match mem.as_ref() {
        Some(m) => m.get_working().len(),
        None => 0,
    }
}

/// Get episode count
pub fn episode_count() -> usize {
    let mem = MEMORY.lock();
    match mem.as_ref() {
        Some(m) => m.episodes.len(),
        None => 0,
    }
}

pub fn init() {
    let mut mem = MEMORY.lock();
    *mem = Some(AgentMemoryInner::new(64, 1024));
    serial_println!(
        "    Memory: working(64 slots) + episodic(1024 episodes), tag-based recall ready"
    );
}

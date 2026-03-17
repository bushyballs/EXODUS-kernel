use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Prompt prefix caching for repeated contexts
///
/// Part of the AIOS LLM layer. Caches the KV-state (key-value pairs
/// from the attention layers) that correspond to common prompt prefixes.
/// When the same prefix is seen again, we skip re-computing those layers
/// and start generation from the cached KV-state.
///
/// The cache uses a hash of the token sequence as a key. Entries are
/// stored in a fixed-size pool and evicted by an LFU (least-frequently-
/// used) policy with an aging mechanism to avoid stale popular entries
/// dominating the cache forever.
///
/// Hash function: FNV-1a 64-bit over the token bytes.
use alloc::vec::Vec;

/// A cached prompt prefix with its KV state
pub struct CacheEntry {
    /// FNV-1a hash of the token prefix sequence
    pub token_hash: u64,
    /// Serialised KV-state for all layers at this prefix
    pub kv_state: Vec<f32>,
    /// How many times this entry has been hit
    pub hit_count: u32,
    /// Number of tokens in the prefix
    pub prefix_len: usize,
    /// Timestamp (monotonic counter) of last access
    pub last_access: u64,
    /// Generation epoch when this entry was created
    pub created_epoch: u64,
}

/// Caches KV states for common prompt prefixes
pub struct PromptCache {
    /// All cache entries
    pub entries: Vec<CacheEntry>,
    /// Maximum number of entries to store
    pub max_entries: usize,
    /// Total lookups performed
    pub total_lookups: u64,
    /// Total hits
    pub total_hits: u64,
    /// Monotonic access counter (serves as a logical clock)
    pub access_counter: u64,
    /// Current epoch for aging
    pub epoch: u64,
    /// Maximum KV-state size per entry (in f32 elements)
    pub max_kv_size: usize,
}

// ── FNV-1a hash ─────────────────────────────────────────────────────

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

/// Compute FNV-1a hash of a token sequence.
pub fn hash_tokens(tokens: &[u32]) -> u64 {
    let mut h = FNV_OFFSET;
    for &tok in tokens {
        // Hash all 4 bytes of each token
        let bytes = tok.to_le_bytes();
        for &b in bytes.iter() {
            h ^= b as u64;
            h = h.wrapping_mul(FNV_PRIME);
        }
    }
    h
}

/// Compute a rolling / incremental hash update. Takes the current hash
/// state and extends it with one more token.
pub fn hash_extend(current: u64, token: u32) -> u64 {
    let mut h = current;
    let bytes = token.to_le_bytes();
    for &b in bytes.iter() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

impl PromptCache {
    /// Create a new prompt cache with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        serial_println!(
            "    [prompt-cache] Creating cache with {} entry slots",
            max_entries
        );
        PromptCache {
            entries: Vec::with_capacity(max_entries),
            max_entries,
            total_lookups: 0,
            total_hits: 0,
            access_counter: 0,
            epoch: 0,
            max_kv_size: 1024 * 1024, // ~4 MB per entry
        }
    }

    /// Look up a prefix by its hash. Returns a reference to the cache
    /// entry if found (and bumps the hit count).
    pub fn lookup(&self, prefix_hash: u64) -> Option<&CacheEntry> {
        // We cannot mutate through &self, but the caller can use
        // `lookup_mut` for hit-count updates. This is a read-only probe.
        for entry in &self.entries {
            if entry.token_hash == prefix_hash {
                return Some(entry);
            }
        }
        None
    }

    /// Look up and update hit statistics (mutable version).
    pub fn lookup_mut(&mut self, prefix_hash: u64) -> Option<&mut CacheEntry> {
        self.total_lookups = self.total_lookups.saturating_add(1);
        self.access_counter = self.access_counter.saturating_add(1);
        let ac = self.access_counter;
        for entry in self.entries.iter_mut() {
            if entry.token_hash == prefix_hash {
                entry.hit_count += 1;
                entry.last_access = ac;
                self.total_hits = self.total_hits.saturating_add(1);
                return Some(entry);
            }
        }
        None
    }

    /// Insert a new prefix into the cache. If the cache is full, the
    /// least-valuable entry is evicted first.
    pub fn insert(&mut self, hash: u64, kv: Vec<f32>) {
        self.insert_with_len(hash, kv, 0);
    }

    /// Insert with explicit prefix length metadata.
    pub fn insert_with_len(&mut self, hash: u64, kv: Vec<f32>, prefix_len: usize) {
        // Check if already present -- update in place
        for entry in self.entries.iter_mut() {
            if entry.token_hash == hash {
                entry.kv_state = kv;
                entry.hit_count += 1;
                entry.last_access = self.access_counter;
                return;
            }
        }

        // Truncate KV-state if it exceeds our budget
        let kv_state = if kv.len() > self.max_kv_size {
            kv[..self.max_kv_size].to_vec()
        } else {
            kv
        };

        // Evict if at capacity
        if self.entries.len() >= self.max_entries {
            self.evict_one();
        }

        self.access_counter = self.access_counter.saturating_add(1);
        let entry = CacheEntry {
            token_hash: hash,
            kv_state,
            hit_count: 1,
            prefix_len,
            last_access: self.access_counter,
            created_epoch: self.epoch,
        };
        self.entries.push(entry);
    }

    /// Evict the least valuable entry using a combined LFU + aging score.
    ///
    /// Score = hit_count * recency_factor
    /// recency_factor decays entries that haven't been accessed recently.
    fn evict_one(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        let current = self.access_counter;
        let mut worst_idx = 0;
        let mut worst_score = u64::MAX;

        for (i, entry) in self.entries.iter().enumerate() {
            // Recency: how many accesses ago was this entry last used?
            let age = current.saturating_sub(entry.last_access);
            // Score: lower is worse. hit_count helps, age hurts.
            let score = (entry.hit_count as u64).saturating_mul(1000) / (age.saturating_add(1));
            if score < worst_score {
                worst_score = score;
                worst_idx = i;
            }
        }

        serial_println!(
            "    [prompt-cache] Evicting entry {} (hash={:#x}, hits={})",
            worst_idx,
            self.entries[worst_idx].token_hash,
            self.entries[worst_idx].hit_count
        );
        self.entries.swap_remove(worst_idx);
    }

    /// Advance the aging epoch. This can be called periodically to
    /// prevent very old entries from dominating.
    pub fn advance_epoch(&mut self) {
        self.epoch = self.epoch.saturating_add(1);
        // Halve hit counts to decay old popularity
        for entry in self.entries.iter_mut() {
            entry.hit_count = entry.hit_count / 2;
        }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_lookups = 0;
        self.total_hits = 0;
    }

    /// Cache hit rate as a percentage (0-100).
    pub fn hit_rate_pct(&self) -> u32 {
        if self.total_lookups == 0 {
            return 0;
        }
        ((self.total_hits * 100) / self.total_lookups) as u32
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Total memory used by KV-states (in f32 elements).
    pub fn total_kv_elements(&self) -> usize {
        self.entries.iter().map(|e| e.kv_state.len()).sum()
    }

    /// Find the longest cached prefix that matches the given tokens.
    /// Tries all prefix lengths from longest to shortest.
    pub fn find_longest_match(&mut self, tokens: &[u32]) -> Option<(usize, &CacheEntry)> {
        // Build hashes for all prefix lengths
        let mut best_len = 0;
        let mut best_hash = 0u64;
        let mut h = FNV_OFFSET;

        for (i, &tok) in tokens.iter().enumerate() {
            h = hash_extend(h, tok);
            // Check if this prefix length is cached
            let found = self.entries.iter().any(|e| e.token_hash == h);
            if found {
                best_len = i + 1;
                best_hash = h;
            }
        }

        if best_len > 0 {
            self.total_lookups = self.total_lookups.saturating_add(1);
            self.total_hits = self.total_hits.saturating_add(1);
            self.access_counter = self.access_counter.saturating_add(1);
            let ac = self.access_counter;
            for entry in self.entries.iter_mut() {
                if entry.token_hash == best_hash {
                    entry.hit_count += 1;
                    entry.last_access = ac;
                    // Return reference: need to re-borrow
                    break;
                }
            }
            // Re-find for the return value (borrow checker)
            for entry in self.entries.iter() {
                if entry.token_hash == best_hash {
                    return Some((best_len, entry));
                }
            }
        }

        None
    }
}

// ── Global Singleton ────────────────────────────────────────────────

struct PromptCacheState {
    cache: PromptCache,
}

static PROMPT_CACHE: Mutex<Option<PromptCacheState>> = Mutex::new(None);

const DEFAULT_MAX_ENTRIES: usize = 64;

pub fn init() {
    let state = PromptCacheState {
        cache: PromptCache::new(DEFAULT_MAX_ENTRIES),
    };
    let mut guard = PROMPT_CACHE.lock();
    *guard = Some(state);
    serial_println!(
        "    [prompt-cache] Subsystem initialised (max_entries={})",
        DEFAULT_MAX_ENTRIES
    );
}

/// Insert a KV-state for a token prefix into the global cache.
pub fn cache_prefix(tokens: &[u32], kv: Vec<f32>) {
    let hash = hash_tokens(tokens);
    let mut guard = PROMPT_CACHE.lock();
    if let Some(state) = guard.as_mut() {
        state.cache.insert_with_len(hash, kv, tokens.len());
    }
}

/// Look up a cached prefix in the global cache.
pub fn lookup_prefix(tokens: &[u32]) -> bool {
    let hash = hash_tokens(tokens);
    let mut guard = PROMPT_CACHE.lock();
    if let Some(state) = guard.as_mut() {
        state.cache.lookup_mut(hash).is_some()
    } else {
        false
    }
}

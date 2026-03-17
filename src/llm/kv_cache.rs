use crate::sync::Mutex;
use alloc::vec;
/// KV-Cache for fast autoregressive generation
///
/// Caches key/value tensors from previous positions so
/// we only compute attention for the new token each step.
///
/// Features:
///   - Per-layer ring-buffer storage with pre-allocated capacity
///   - Oldest-first and attention-score-based eviction policies
///   - Sequence position tracking for absolute position recovery
///   - Cache statistics (hit rate, memory, occupancy)
///   - Multi-sequence (batch) support for parallel generation
///   - Sliding window for bounded memory
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

use super::transformer::Q16;

/// Eviction policy when the cache is full
#[derive(Clone, Copy, PartialEq)]
pub enum EvictionPolicy {
    /// Remove the oldest entry (FIFO ring buffer)
    OldestFirst,
    /// Remove the entry with the lowest accumulated attention score
    LowestAttention,
}

/// Statistics for a single layer cache
#[derive(Clone, Copy)]
pub struct LayerCacheStats {
    pub total_inserts: u64,
    pub total_evictions: u64,
    pub current_len: u32,
    pub capacity: u32,
}

/// Cache for one layer's keys and values using a ring buffer
struct LayerCache {
    /// Pre-allocated key storage: [capacity][kv_dim]
    /// Stored flat: position p starts at p * kv_dim
    keys: Vec<Q16>,
    /// Pre-allocated value storage: same layout as keys
    values: Vec<Q16>,
    /// Absolute sequence position for each cached slot
    positions: Vec<u32>,
    /// Accumulated attention scores for eviction (Q16)
    attention_scores: Vec<Q16>,
    /// Ring buffer write pointer (next slot to write)
    write_ptr: u32,
    /// Number of valid entries (grows up to capacity, then stays fixed)
    len: u32,
    /// Maximum number of positions that can be cached
    capacity: u32,
    /// Dimension of each KV vector (head_dim * n_kv_heads)
    kv_dim: u32,
    /// Statistics
    total_inserts: u64,
    total_evictions: u64,
}

/// Per-sequence cache state for batch generation
struct SequenceState {
    /// Unique sequence ID
    seq_id: u32,
    /// Current generation position in this sequence
    current_pos: u32,
    /// Starting position in the shared cache (for isolation)
    cache_start: u32,
    /// Number of positions this sequence occupies
    cache_len: u32,
    /// Whether this sequence is active
    active: bool,
}

/// Full KV-cache across all layers and sequences
struct KvCache {
    layers: Vec<LayerCache>,
    n_layers: u32,
    head_dim: u32,
    n_kv_heads: u32,
    max_seq_len: u32,
    sliding_window: u32, // 0 = unlimited
    eviction_policy: EvictionPolicy,
    /// Multi-sequence tracking
    sequences: Vec<SequenceState>,
    next_seq_id: u32,
    max_sequences: u32,
    /// Global statistics
    total_cached_tokens: u64,
    total_cache_hits: u64,
    total_cache_misses: u64,
    memory_used_bytes: u64,
}

static KV_CACHE: Mutex<Option<KvCache>> = Mutex::new(None);

impl LayerCache {
    /// Create a new layer cache with pre-allocated storage
    fn new(capacity: u32, kv_dim: u32) -> Self {
        let total_elements = capacity as usize * kv_dim as usize;
        LayerCache {
            keys: vec![0; total_elements],
            values: vec![0; total_elements],
            positions: vec![0; capacity as usize],
            attention_scores: vec![0; capacity as usize],
            write_ptr: 0,
            len: 0,
            capacity,
            kv_dim,
            total_inserts: 0,
            total_evictions: 0,
        }
    }

    /// Insert a key-value pair at the next available slot.
    /// If full, evicts according to the given policy.
    fn push(&mut self, key: &[Q16], value: &[Q16], position: u32, policy: EvictionPolicy) {
        let dim = self.kv_dim as usize;
        if key.len() < dim || value.len() < dim {
            return;
        }

        let slot = if self.len < self.capacity {
            // Cache not full yet: use the next slot
            let s = self.len;
            self.len += 1;
            s
        } else {
            // Cache full: evict
            self.total_evictions = self.total_evictions.saturating_add(1);
            match policy {
                EvictionPolicy::OldestFirst => {
                    // Ring buffer: overwrite at write_ptr (oldest in circular order)
                    self.write_ptr % self.capacity
                }
                EvictionPolicy::LowestAttention => self.find_lowest_attention_slot(),
            }
        };

        // Write key and value into the pre-allocated buffer
        let offset = slot as usize * dim;
        self.keys[offset..offset + dim].copy_from_slice(&key[..dim]);
        self.values[offset..offset + dim].copy_from_slice(&value[..dim]);
        self.positions[slot as usize] = position;
        self.attention_scores[slot as usize] = 0; // Reset score for new entry

        // Advance ring-buffer pointer
        self.write_ptr = (slot + 1) % self.capacity;
        self.total_inserts = self.total_inserts.saturating_add(1);
    }

    /// Find the slot with the lowest accumulated attention score
    fn find_lowest_attention_slot(&self) -> u32 {
        if self.len == 0 {
            return 0;
        }
        let mut min_score = i32::MAX;
        let mut min_slot: u32 = 0;
        for i in 0..self.len {
            if self.attention_scores[i as usize] < min_score {
                min_score = self.attention_scores[i as usize];
                min_slot = i;
            }
        }
        min_slot
    }

    /// Update attention score for a cached position.
    /// Called after attention computation to track which entries matter.
    fn update_attention_score(&mut self, slot: u32, score: Q16) {
        if (slot as usize) < self.len as usize {
            // Exponential moving average: new = old * 0.875 + score * 0.125
            let old = self.attention_scores[slot as usize];
            let decay = (old as i64 * 57344) >> 16; // 0.875 in Q16 = 57344
            let new_part = (score as i64 * 8192) >> 16; // 0.125 in Q16 = 8192
            self.attention_scores[slot as usize] = (decay + new_part) as Q16;
        }
    }

    /// Get the key vector for a particular cached slot
    fn get_key(&self, slot: u32) -> &[Q16] {
        let dim = self.kv_dim as usize;
        let offset = slot as usize * dim;
        &self.keys[offset..offset + dim]
    }

    /// Get the value vector for a particular cached slot
    fn get_value(&self, slot: u32) -> &[Q16] {
        let dim = self.kv_dim as usize;
        let offset = slot as usize * dim;
        &self.values[offset..offset + dim]
    }

    /// Get the absolute sequence position of a cached slot
    fn get_position(&self, slot: u32) -> u32 {
        self.positions[slot as usize]
    }

    /// Return number of valid entries
    fn current_len(&self) -> u32 {
        self.len
    }

    /// Collect all valid keys as a flat contiguous buffer reference.
    /// Returns (data_slice, n_entries, kv_dim).
    fn all_keys(&self) -> (&[Q16], u32, u32) {
        let end = self.len as usize * self.kv_dim as usize;
        (&self.keys[..end], self.len, self.kv_dim)
    }

    /// Collect all valid values as a flat contiguous buffer reference.
    fn all_values(&self) -> (&[Q16], u32, u32) {
        let end = self.len as usize * self.kv_dim as usize;
        (&self.values[..end], self.len, self.kv_dim)
    }

    /// Get all valid positions as a slice
    fn all_positions(&self) -> &[u32] {
        &self.positions[..self.len as usize]
    }

    /// Clear all cached data but keep allocations
    fn clear(&mut self) {
        // Zero out only the used portion
        let used = self.len as usize * self.kv_dim as usize;
        for i in 0..used {
            self.keys[i] = 0;
            self.values[i] = 0;
        }
        for i in 0..self.len as usize {
            self.positions[i] = 0;
            self.attention_scores[i] = 0;
        }
        self.len = 0;
        self.write_ptr = 0;
    }

    /// Evict entries older than the given position (for sliding window)
    fn evict_before(&mut self, min_position: u32) {
        // Compact: shift valid entries to the front
        let dim = self.kv_dim as usize;
        let mut write = 0u32;
        for read in 0..self.len {
            if self.positions[read as usize] >= min_position {
                if write != read {
                    // Copy key
                    let src_off = read as usize * dim;
                    let dst_off = write as usize * dim;
                    for d in 0..dim {
                        self.keys[dst_off + d] = self.keys[src_off + d];
                        self.values[dst_off + d] = self.values[src_off + d];
                    }
                    self.positions[write as usize] = self.positions[read as usize];
                    self.attention_scores[write as usize] = self.attention_scores[read as usize];
                }
                write += 1;
            } else {
                self.total_evictions = self.total_evictions.saturating_add(1);
            }
        }
        self.len = write;
        self.write_ptr = write % self.capacity;
    }

    /// Get layer statistics
    fn stats(&self) -> LayerCacheStats {
        LayerCacheStats {
            total_inserts: self.total_inserts,
            total_evictions: self.total_evictions,
            current_len: self.len,
            capacity: self.capacity,
        }
    }

    /// Memory used by this layer in bytes
    fn memory_bytes(&self) -> u64 {
        let dim = self.kv_dim as u64;
        let cap = self.capacity as u64;
        // keys + values (Q16 = 4 bytes each) + positions (4 bytes) + scores (4 bytes)
        cap * dim * 4 * 2 + cap * 4 * 2
    }
}

impl KvCache {
    fn new(
        n_layers: u32,
        head_dim: u32,
        n_kv_heads: u32,
        max_seq: u32,
        sliding_window: u32,
        policy: EvictionPolicy,
        max_sequences: u32,
    ) -> Self {
        let capacity = if sliding_window > 0 {
            sliding_window.min(max_seq)
        } else {
            max_seq
        };
        let kv_dim = head_dim * n_kv_heads;

        let mut layers = Vec::with_capacity(n_layers as usize);
        for _ in 0..n_layers {
            layers.push(LayerCache::new(capacity, kv_dim));
        }

        let mut memory_used_bytes: u64 = 0;
        for layer in &layers {
            memory_used_bytes += layer.memory_bytes();
        }

        KvCache {
            layers,
            n_layers,
            head_dim,
            n_kv_heads,
            max_seq_len: max_seq,
            sliding_window,
            eviction_policy: policy,
            sequences: Vec::new(),
            next_seq_id: 0,
            max_sequences,
            total_cached_tokens: 0,
            total_cache_hits: 0,
            total_cache_misses: 0,
            memory_used_bytes,
        }
    }

    /// Cache a key-value pair for a specific layer at a given sequence position
    fn cache_token(&mut self, layer: u32, key: Vec<Q16>, value: Vec<Q16>, position: u32) {
        if (layer as usize) < self.layers.len() {
            self.layers[layer as usize].push(&key, &value, position, self.eviction_policy);
            self.total_cached_tokens = self.total_cached_tokens.saturating_add(1);

            // If sliding window is active, evict old entries
            if self.sliding_window > 0 && position >= self.sliding_window {
                let min_pos = position - self.sliding_window + 1;
                self.layers[layer as usize].evict_before(min_pos);
            }
        }
    }

    /// Cache a key-value pair using the old API (no explicit position)
    fn cache_token_simple(&mut self, layer: u32, key: Vec<Q16>, value: Vec<Q16>) {
        let pos = self.current_len();
        self.cache_token(layer, key, value, pos);
    }

    /// Retrieve cached keys for a layer (flat buffer, n_entries, dim)
    fn get_layer_keys(&self, layer: u32) -> Option<(&[Q16], u32, u32)> {
        self.layers.get(layer as usize).map(|l| l.all_keys())
    }

    /// Retrieve cached values for a layer
    fn get_layer_values(&self, layer: u32) -> Option<(&[Q16], u32, u32)> {
        self.layers.get(layer as usize).map(|l| l.all_values())
    }

    /// Get a specific key vector from the cache
    fn get_key_at(&self, layer: u32, slot: u32) -> Option<&[Q16]> {
        self.layers.get(layer as usize).and_then(|l| {
            if slot < l.len {
                Some(l.get_key(slot))
            } else {
                None
            }
        })
    }

    /// Get a specific value vector from the cache
    fn get_value_at(&self, layer: u32, slot: u32) -> Option<&[Q16]> {
        self.layers.get(layer as usize).and_then(|l| {
            if slot < l.len {
                Some(l.get_value(slot))
            } else {
                None
            }
        })
    }

    /// Update attention scores after computing attention for a layer.
    /// `scores` maps slot index -> attention weight received at that slot.
    fn update_scores(&mut self, layer: u32, scores: &[(u32, Q16)]) {
        if let Some(layer_cache) = self.layers.get_mut(layer as usize) {
            for &(slot, score) in scores {
                layer_cache.update_attention_score(slot, score);
            }
        }
    }

    /// Record a cache hit (position was already cached)
    fn record_hit(&mut self) {
        self.total_cache_hits = self.total_cache_hits.saturating_add(1);
    }

    /// Record a cache miss (position not in cache)
    fn record_miss(&mut self) {
        self.total_cache_misses = self.total_cache_misses.saturating_add(1);
    }

    /// Get the layer cache directly (immutable)
    fn get_layer_cache(&self, layer: u32) -> Option<&LayerCache> {
        self.layers.get(layer as usize)
    }

    /// Allocate a new sequence for batch generation.
    /// Returns the sequence ID, or None if at max capacity.
    fn alloc_sequence(&mut self) -> Option<u32> {
        if self.sequences.len() as u32 >= self.max_sequences {
            // Try to reuse an inactive sequence
            for seq in &mut self.sequences {
                if !seq.active {
                    seq.active = true;
                    seq.current_pos = 0;
                    seq.cache_len = 0;
                    return Some(seq.seq_id);
                }
            }
            return None;
        }
        let id = self.next_seq_id;
        self.next_seq_id = self.next_seq_id.saturating_add(1);
        self.sequences.push(SequenceState {
            seq_id: id,
            current_pos: 0,
            cache_start: 0,
            cache_len: 0,
            active: true,
        });
        Some(id)
    }

    /// Release a sequence, marking it inactive
    fn free_sequence(&mut self, seq_id: u32) {
        for seq in &mut self.sequences {
            if seq.seq_id == seq_id {
                seq.active = false;
                break;
            }
        }
    }

    /// Advance a sequence's position counter
    fn advance_sequence(&mut self, seq_id: u32) {
        for seq in &mut self.sequences {
            if seq.seq_id == seq_id && seq.active {
                seq.current_pos += 1;
                seq.cache_len += 1;
                break;
            }
        }
    }

    /// Get a sequence's current position
    fn sequence_position(&self, seq_id: u32) -> Option<u32> {
        self.sequences
            .iter()
            .find(|s| s.seq_id == seq_id && s.active)
            .map(|s| s.current_pos)
    }

    /// Number of active sequences
    fn active_sequences(&self) -> u32 {
        self.sequences.iter().filter(|s| s.active).count() as u32
    }

    /// Clear all cached data across all layers and sequences
    fn clear(&mut self) {
        for layer in &mut self.layers {
            layer.clear();
        }
        for seq in &mut self.sequences {
            seq.active = false;
            seq.current_pos = 0;
            seq.cache_len = 0;
        }
        self.total_cached_tokens = 0;
        self.total_cache_hits = 0;
        self.total_cache_misses = 0;
    }

    /// Clear cache for a single layer
    fn clear_layer(&mut self, layer: u32) {
        if let Some(l) = self.layers.get_mut(layer as usize) {
            l.clear();
        }
    }

    /// Current number of cached positions (from first layer)
    fn current_len(&self) -> u32 {
        self.layers.first().map_or(0, |l| l.len)
    }

    /// Set the eviction policy
    fn set_eviction_policy(&mut self, policy: EvictionPolicy) {
        self.eviction_policy = policy;
    }

    /// Change the sliding window size (0 = unlimited)
    fn set_sliding_window(&mut self, window: u32) {
        self.sliding_window = window;
    }

    /// Get occupancy as percentage (0-100)
    fn occupancy_percent(&self) -> u32 {
        let cap = self.layers.first().map_or(1, |l| l.capacity);
        let len = self.current_len();
        if cap == 0 {
            return 0;
        }
        (len * 100) / cap
    }

    /// Get cache hit rate as Q16 fraction (0.0 to 1.0)
    fn hit_rate_q16(&self) -> Q16 {
        let total = self.total_cache_hits + self.total_cache_misses;
        if total == 0 {
            return 0;
        }
        ((self.total_cache_hits as i64 * 65536) / total as i64) as Q16
    }

    /// Total pre-allocated memory in megabytes
    fn memory_mb(&self) -> u32 {
        (self.memory_used_bytes / (1024 * 1024)) as u32
    }

    /// Per-layer statistics
    fn layer_stats(&self, layer: u32) -> Option<LayerCacheStats> {
        self.layers.get(layer as usize).map(|l| l.stats())
    }

    /// Summary of all layers
    fn total_entries(&self) -> u64 {
        self.layers.iter().map(|l| l.len as u64).sum()
    }

    /// Check if a specific position is cached in a layer
    fn has_position(&self, layer: u32, position: u32) -> bool {
        if let Some(l) = self.layers.get(layer as usize) {
            for i in 0..l.len {
                if l.positions[i as usize] == position {
                    return true;
                }
            }
        }
        false
    }

    /// Find the cache slot for a given position in a layer
    fn find_position(&self, layer: u32, position: u32) -> Option<u32> {
        if let Some(l) = self.layers.get(layer as usize) {
            for i in 0..l.len {
                if l.positions[i as usize] == position {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Get all cached positions for a layer, sorted
    fn cached_positions(&self, layer: u32) -> Vec<u32> {
        if let Some(l) = self.layers.get(layer as usize) {
            let mut positions: Vec<u32> = l.all_positions().to_vec();
            // Simple insertion sort (positions are mostly sorted)
            for i in 1..positions.len() {
                let key = positions[i];
                let mut j = i;
                while j > 0 && positions[j - 1] > key {
                    positions[j] = positions[j - 1];
                    j -= 1;
                }
                positions[j] = key;
            }
            positions
        } else {
            Vec::new()
        }
    }

    /// Trim cache to only keep the most recent `keep` entries (by position)
    fn trim_to(&mut self, keep: u32) {
        for layer in &mut self.layers {
            if layer.len <= keep {
                continue;
            }
            // Find the minimum position to keep
            // We need the (len - keep)-th smallest position
            let mut positions: Vec<u32> = layer.all_positions().to_vec();
            // Simple sort
            for i in 1..positions.len() {
                let key = positions[i];
                let mut j = i;
                while j > 0 && positions[j - 1] > key {
                    positions[j] = positions[j - 1];
                    j -= 1;
                }
                positions[j] = key;
            }
            let cutoff_idx = (layer.len - keep) as usize;
            if cutoff_idx < positions.len() {
                let min_pos = positions[cutoff_idx];
                layer.evict_before(min_pos);
            }
        }
    }
}

// =============================================================================
// Public API
// =============================================================================

/// Initialize the KV cache with default configuration
pub fn init() {
    let mut kv = KV_CACHE.lock();
    *kv = Some(KvCache::new(
        12,   // n_layers
        64,   // head_dim
        12,   // n_kv_heads
        8192, // max_seq_len
        0,    // sliding_window (0 = unlimited)
        EvictionPolicy::OldestFirst,
        4, // max_sequences
    ));
    serial_println!("    KV-cache: ring-buffer, eviction, multi-seq, score tracking ready");
}

/// Initialize with custom configuration
pub fn init_with_config(
    n_layers: u32,
    head_dim: u32,
    n_kv_heads: u32,
    max_seq: u32,
    sliding_window: u32,
) {
    let mut kv = KV_CACHE.lock();
    *kv = Some(KvCache::new(
        n_layers,
        head_dim,
        n_kv_heads,
        max_seq,
        sliding_window,
        EvictionPolicy::OldestFirst,
        4,
    ));
}

/// Cache a key-value pair for the given layer and position
pub fn cache_kv(layer: u32, key: Vec<Q16>, value: Vec<Q16>, position: u32) {
    if let Some(cache) = KV_CACHE.lock().as_mut() {
        cache.cache_token(layer, key, value, position);
    }
}

/// Get cached sequence length (number of cached positions)
pub fn cached_len() -> u32 {
    KV_CACHE.lock().as_ref().map_or(0, |c| c.current_len())
}

/// Clear all cached data
pub fn clear() {
    if let Some(cache) = KV_CACHE.lock().as_mut() {
        cache.clear();
    }
}

/// Get occupancy percentage
pub fn occupancy() -> u32 {
    KV_CACHE
        .lock()
        .as_ref()
        .map_or(0, |c| c.occupancy_percent())
}

/// Get memory usage in MB
pub fn memory_mb() -> u32 {
    KV_CACHE.lock().as_ref().map_or(0, |c| c.memory_mb())
}

/// Get total entries across all layers
pub fn total_entries() -> u64 {
    KV_CACHE.lock().as_ref().map_or(0, |c| c.total_entries())
}

/// Allocate a new generation sequence
pub fn alloc_sequence() -> Option<u32> {
    KV_CACHE.lock().as_mut().and_then(|c| c.alloc_sequence())
}

/// Free a generation sequence
pub fn free_sequence(seq_id: u32) {
    if let Some(cache) = KV_CACHE.lock().as_mut() {
        cache.free_sequence(seq_id);
    }
}

/// Set eviction policy
pub fn set_eviction_policy(policy: EvictionPolicy) {
    if let Some(cache) = KV_CACHE.lock().as_mut() {
        cache.set_eviction_policy(policy);
    }
}

/// Trim cache to most recent N entries
pub fn trim_to(keep: u32) {
    if let Some(cache) = KV_CACHE.lock().as_mut() {
        cache.trim_to(keep);
    }
}

/// Text Embeddings — vector store, similarity search, and clustering
///
/// Provides a fully local embedding and retrieval system for the
/// Hoags AI. Converts token sequences into fixed-dimension Q16
/// vectors, stores them in an indexed vector store, and supports
/// cosine similarity search, k-nearest-neighbor lookup, semantic
/// clustering, and incremental index building.
///
/// All math is integer/fixed-point (Q16). No floats, no BLAS,
/// no external libraries. Built for bare-metal x86_64.
///
/// Features:
///   - Token-to-vector embedding projection
///   - Vector store with ID-based lookup
///   - Cosine similarity in Q16 fixed-point
///   - K-nearest-neighbor semantic search
///   - Simple k-means clustering
///   - Incremental index building and compaction
///   - Memory-efficient storage with dimensionality reduction

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

use super::transformer::{Q16, q16_mul, q16_from_int};

// ── Constants ────────────────────────────────────────────────────────

/// Embedding dimension (must be power of 2 for efficient ops)
const EMBED_DIM: usize = 128;

/// Maximum number of stored vectors
const MAX_VECTORS: usize = 8192;

/// Maximum search results returned
const MAX_SEARCH_RESULTS: usize = 32;

/// Maximum number of clusters
const MAX_CLUSTERS: usize = 64;

/// K-means iteration limit
const KMEANS_MAX_ITERS: u32 = 50;

/// Convergence threshold in Q16 (~0.001)
const CONVERGENCE_THRESHOLD: Q16 = 66; // 0.001 * 65536

/// Q16 one constant
const Q16_ONE: i32 = 65536;

/// Minimum vector magnitude squared to avoid division by zero
const MIN_MAGNITUDE_SQ: i64 = 1;

// ── Types ────────────────────────────────────────────────────────────

/// A stored embedding vector with metadata
#[derive(Clone)]
pub struct EmbeddingEntry {
    pub id: u32,
    pub source_hash: u64,
    pub vector: Vec<Q16>,
    pub magnitude_sq: i64,
    pub cluster_id: u32,
    pub timestamp: u64,
    pub access_count: u32,
}

impl EmbeddingEntry {
    fn new(id: u32, source: u64, vector: Vec<Q16>, timestamp: u64) -> Self {
        let mag_sq = compute_magnitude_sq(&vector);
        EmbeddingEntry {
            id,
            source_hash: source,
            vector,
            magnitude_sq: mag_sq,
            cluster_id: 0,
            timestamp,
            access_count: 0,
        }
    }
}

/// A search result with similarity score
#[derive(Clone, Copy)]
pub struct SearchResult {
    pub id: u32,
    pub source_hash: u64,
    pub similarity: Q16,
}

/// A cluster centroid
#[derive(Clone)]
pub struct Cluster {
    pub id: u32,
    pub centroid: Vec<Q16>,
    pub member_count: u32,
    pub coherence: Q16,
}

/// Index status
#[derive(Clone, Copy, PartialEq)]
pub enum IndexStatus {
    Empty,
    Building,
    Ready,
    NeedsRebuild,
}

// ── Math Helpers ─────────────────────────────────────────────────────

/// Compute the squared magnitude of a Q16 vector
/// Returns a raw i64 to avoid overflow: sum of (v[i]^2 >> 16)
fn compute_magnitude_sq(v: &[Q16]) -> i64 {
    let mut sum: i64 = 0;
    for &x in v {
        sum += ((x as i64) * (x as i64)) >> 16;
    }
    sum.max(MIN_MAGNITUDE_SQ)
}

/// Compute dot product of two Q16 vectors
/// Returns Q16 result: sum of (a[i] * b[i]) >> 16
fn dot_product(a: &[Q16], b: &[Q16]) -> i64 {
    let len = a.len().min(b.len());
    let mut sum: i64 = 0;
    for i in 0..len {
        sum += ((a[i] as i64) * (b[i] as i64)) >> 16;
    }
    sum
}

/// Compute cosine similarity between two vectors in Q16
/// Returns Q16 value in range [-1.0, 1.0] (i.e., [-65536, 65536])
fn cosine_similarity(a: &[Q16], a_mag_sq: i64, b: &[Q16], b_mag_sq: i64) -> Q16 {
    let dot = dot_product(a, b);

    // Approximate sqrt using integer Newton's method
    let mag_product_sq = (a_mag_sq >> 8) * (b_mag_sq >> 8); // scale down to avoid overflow
    if mag_product_sq <= 0 {
        return 0;
    }

    let mag_product = isqrt(mag_product_sq);
    if mag_product == 0 {
        return 0;
    }

    // Scale dot product to match the magnitude scaling
    let dot_scaled = dot >> 8;

    // cosine = dot / mag_product, result in Q16
    let result = (((dot_scaled) << 16) / mag_product) as Q16;

    // Clamp to [-1.0, 1.0] in Q16
    result.max(-Q16_ONE).min(Q16_ONE)
}

/// Integer square root using Newton's method
fn isqrt(n: i64) -> i64 {
    if n <= 0 { return 0; }
    if n == 1 { return 1; }

    let mut x = n;
    let mut y = (x + 1) >> 1;

    // Limit iterations to prevent spinning
    let mut iters = 0;
    while y < x && iters < 64 {
        x = y;
        y = (x + n / x) >> 1;
        iters += 1;
    }

    x
}

/// Add two Q16 vectors element-wise (a += b)
fn vec_add(a: &mut [Q16], b: &[Q16]) {
    let len = a.len().min(b.len());
    for i in 0..len {
        a[i] = a[i].saturating_add(b[i]);
    }
}

/// Scale a Q16 vector by an integer divisor (in-place)
fn vec_div_scalar(v: &mut [Q16], divisor: i32) {
    if divisor <= 0 { return; }
    for x in v.iter_mut() {
        *x = *x / divisor;
    }
}

// ── Embedding Engine ─────────────────────────────────────────────────

struct EmbeddingEngine {
    store: Vec<EmbeddingEntry>,
    clusters: Vec<Cluster>,
    index_status: IndexStatus,
    next_id: u32,
    embed_dim: usize,
    total_searches: u64,
    total_insertions: u64,
    // Simple projection weights (hash-seeded pseudo-random matrix)
    projection_seed: u64,
}

impl EmbeddingEngine {
    fn new() -> Self {
        EmbeddingEngine {
            store: Vec::new(),
            clusters: Vec::new(),
            index_status: IndexStatus::Empty,
            next_id: 1,
            embed_dim: EMBED_DIM,
            total_searches: 0,
            total_insertions: 0,
            projection_seed: 0xEBED_D14C_CAFE_0001,
        }
    }

    // ── Embedding Generation ─────────────────────────────────────────

    /// Project a token sequence into an embedding vector
    /// Uses a deterministic hash-based projection (pseudo-random matrix)
    fn embed_tokens(&self, tokens: &[u32]) -> Vec<Q16> {
        let mut vector = vec![0i32; self.embed_dim];

        for (t_idx, &token) in tokens.iter().enumerate() {
            let token_val = token as u64;
            let pos_val = t_idx as u64;

            for d in 0..self.embed_dim {
                // Deterministic pseudo-random projection weight
                let mut h = self.projection_seed;
                h ^= token_val.wrapping_mul(0x0101_0101_0101_0101);
                h ^= pos_val.wrapping_mul(0x00FF_00FF_00FF_00FF);
                h ^= (d as u64).wrapping_mul(0x5555_5555_5555_5555);
                h ^= h >> 33;
                h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
                h ^= h >> 33;

                // Convert hash to a small Q16 weight in range [-1, 1]
                let weight = ((h as i64 % (2 * Q16_ONE as i64)) - Q16_ONE as i64) as Q16;
                // Scale down per-token contribution
                let contrib = weight / (tokens.len().max(1) as i32);
                vector[d] = vector[d].saturating_add(contrib);
            }
        }

        vector
    }

    // ── Vector Store Operations ──────────────────────────────────────

    /// Insert a new embedding into the store
    fn insert(&mut self, source: u64, tokens: &[u32], timestamp: u64) -> u32 {
        let vector = self.embed_tokens(tokens);
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        // Evict oldest if at capacity
        if self.store.len() >= MAX_VECTORS {
            self.evict_oldest();
        }

        let entry = EmbeddingEntry::new(id, source, vector, timestamp);
        self.store.push(entry);
        self.total_insertions = self.total_insertions.saturating_add(1);
        self.index_status = IndexStatus::NeedsRebuild;

        id
    }

    /// Insert a pre-computed vector
    fn insert_vector(&mut self, source: u64, vector: Vec<Q16>, timestamp: u64) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        if self.store.len() >= MAX_VECTORS {
            self.evict_oldest();
        }

        let entry = EmbeddingEntry::new(id, source, vector, timestamp);
        self.store.push(entry);
        self.total_insertions = self.total_insertions.saturating_add(1);
        self.index_status = IndexStatus::NeedsRebuild;

        id
    }

    /// Remove a vector by ID
    fn remove(&mut self, id: u32) -> bool {
        let before = self.store.len();
        self.store.retain(|e| e.id != id);
        let removed = self.store.len() < before;
        if removed {
            self.index_status = IndexStatus::NeedsRebuild;
        }
        removed
    }

    /// Evict the oldest entry by timestamp
    fn evict_oldest(&mut self) {
        if self.store.is_empty() { return; }

        let mut oldest_ts: u64 = u64::MAX;
        let mut oldest_idx: usize = 0;

        for (i, entry) in self.store.iter().enumerate() {
            if entry.timestamp < oldest_ts {
                oldest_ts = entry.timestamp;
                oldest_idx = i;
            }
        }

        self.store.remove(oldest_idx);
    }

    // ── Search ───────────────────────────────────────────────────────

    /// Search for the k most similar vectors to a query
    fn search(&mut self, query_tokens: &[u32], k: usize) -> Vec<SearchResult> {
        let query_vec = self.embed_tokens(query_tokens);
        self.search_by_vector(&query_vec, k)
    }

    /// Search by a pre-computed query vector
    fn search_by_vector(&mut self, query: &[Q16], k: usize) -> Vec<SearchResult> {
        self.total_searches = self.total_searches.saturating_add(1);
        let k = k.min(MAX_SEARCH_RESULTS);
        let query_mag_sq = compute_magnitude_sq(query);

        let mut results: Vec<SearchResult> = Vec::with_capacity(self.store.len());

        for entry in &mut self.store {
            let sim = cosine_similarity(query, query_mag_sq, &entry.vector, entry.magnitude_sq);
            results.push(SearchResult {
                id: entry.id,
                source_hash: entry.source_hash,
                similarity: sim,
            });
            entry.access_count = entry.access_count.saturating_add(1);
        }

        // Sort by similarity descending (selection sort for top-k)
        for i in 0..k.min(results.len()) {
            let mut best = i;
            for j in (i + 1)..results.len() {
                if results[j].similarity > results[best].similarity {
                    best = j;
                }
            }
            if best != i {
                results.swap(i, best);
            }
        }

        results.truncate(k);
        results
    }

    /// Search within a specific cluster only
    fn search_in_cluster(&mut self, query: &[Q16], cluster_id: u32, k: usize) -> Vec<SearchResult> {
        self.total_searches = self.total_searches.saturating_add(1);
        let k = k.min(MAX_SEARCH_RESULTS);
        let query_mag_sq = compute_magnitude_sq(query);

        let mut results: Vec<SearchResult> = Vec::new();

        for entry in &mut self.store {
            if entry.cluster_id != cluster_id { continue; }
            let sim = cosine_similarity(query, query_mag_sq, &entry.vector, entry.magnitude_sq);
            results.push(SearchResult {
                id: entry.id,
                source_hash: entry.source_hash,
                similarity: sim,
            });
            entry.access_count = entry.access_count.saturating_add(1);
        }

        // Sort descending
        for i in 0..k.min(results.len()) {
            let mut best = i;
            for j in (i + 1)..results.len() {
                if results[j].similarity > results[best].similarity {
                    best = j;
                }
            }
            if best != i {
                results.swap(i, best);
            }
        }

        results.truncate(k);
        results
    }

    // ── Clustering (K-Means) ─────────────────────────────────────────

    /// Run k-means clustering on the stored vectors
    fn cluster(&mut self, k: u32) {
        let k = (k as usize).min(MAX_CLUSTERS).min(self.store.len());
        if k == 0 || self.store.is_empty() { return; }

        // Initialize centroids from first k entries
        let mut centroids: Vec<Vec<Q16>> = Vec::with_capacity(k);
        for i in 0..k {
            centroids.push(self.store[i].vector.clone());
        }

        for _iter in 0..KMEANS_MAX_ITERS {
            // Assignment step: assign each vector to nearest centroid
            for entry in &mut self.store {
                let mut best_cluster: u32 = 0;
                let mut best_sim: Q16 = i32::MIN;

                for (c_idx, centroid) in centroids.iter().enumerate() {
                    let c_mag_sq = compute_magnitude_sq(centroid);
                    let sim = cosine_similarity(
                        &entry.vector, entry.magnitude_sq,
                        centroid, c_mag_sq,
                    );
                    if sim > best_sim {
                        best_sim = sim;
                        best_cluster = c_idx as u32;
                    }
                }

                entry.cluster_id = best_cluster;
            }

            // Update step: recompute centroids
            let mut new_centroids: Vec<Vec<Q16>> = Vec::with_capacity(k);
            let mut counts: Vec<i32> = Vec::with_capacity(k);

            for _ in 0..k {
                new_centroids.push(vec![0i32; self.embed_dim]);
                counts.push(0);
            }

            for entry in &self.store {
                let c = entry.cluster_id as usize;
                if c < k {
                    vec_add(&mut new_centroids[c], &entry.vector);
                    counts[c] += 1;
                }
            }

            // Average
            let mut max_shift: Q16 = 0;
            for c in 0..k {
                if counts[c] > 0 {
                    vec_div_scalar(&mut new_centroids[c], counts[c]);
                }

                // Compute shift from old centroid
                let shift = dot_product(&new_centroids[c], &centroids[c]);
                let shift_delta = (Q16_ONE as i64 - shift.abs()).abs() as Q16;
                if shift_delta > max_shift {
                    max_shift = shift_delta;
                }
            }

            centroids = new_centroids;

            // Check convergence
            if max_shift < CONVERGENCE_THRESHOLD {
                break;
            }
        }

        // Store final clusters
        self.clusters.clear();
        for (c_idx, centroid) in centroids.into_iter().enumerate() {
            let member_count = self.store.iter()
                .filter(|e| e.cluster_id == c_idx as u32)
                .count() as u32;

            self.clusters.push(Cluster {
                id: c_idx as u32,
                centroid,
                member_count,
                coherence: 0, // Computed below
            });
        }

        // Compute cluster coherence (average intra-cluster similarity)
        for cluster in &mut self.clusters {
            let c_mag_sq = compute_magnitude_sq(&cluster.centroid);
            let mut sim_sum: i64 = 0;
            let mut count: i32 = 0;

            for entry in &self.store {
                if entry.cluster_id == cluster.id {
                    let sim = cosine_similarity(
                        &entry.vector, entry.magnitude_sq,
                        &cluster.centroid, c_mag_sq,
                    );
                    sim_sum += sim as i64;
                    count += 1;
                }
            }

            if count > 0 {
                cluster.coherence = (sim_sum / count as i64) as Q16;
            }
        }

        self.index_status = IndexStatus::Ready;
    }

    // ── Index Management ─────────────────────────────────────────────

    /// Rebuild the index (re-cluster)
    fn rebuild_index(&mut self) {
        self.index_status = IndexStatus::Building;
        let k = (self.store.len() / 64).max(4).min(MAX_CLUSTERS) as u32;
        self.cluster(k);
    }

    /// Compact the store by removing low-access entries
    fn compact(&mut self, min_access: u32) {
        self.store.retain(|e| e.access_count >= min_access);
        self.index_status = IndexStatus::NeedsRebuild;
    }

    /// Get engine statistics
    fn get_stats(&self) -> (u32, u64, u64, u32, IndexStatus) {
        (
            self.store.len() as u32,
            self.total_insertions,
            self.total_searches,
            self.clusters.len() as u32,
            self.index_status,
        )
    }
}

// ── Global State ─────────────────────────────────────────────────────

static ENGINE: Mutex<Option<EmbeddingEngine>> = Mutex::new(None);

/// Access the global embedding engine
pub fn with_engine<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut EmbeddingEngine) -> R,
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
    let mut e = ENGINE.lock();
    *e = Some(EmbeddingEngine::new());
    serial_println!("    Embeddings: {}D vectors, cosine similarity, k-NN search, k-means clustering ready", EMBED_DIM);
}

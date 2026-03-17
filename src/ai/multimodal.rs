use crate::sync::Mutex;
/// Multimodal AI fusion for Genesis
///
/// Text+image+audio feature combining, cross-modal attention,
/// and unified embedding — all on-device with Q16 fixed-point math.
///
/// Features:
///   - Weighted, attention, concatenation, and gated fusion strategies
///   - Cross-modal search: text->image, image->text, text->audio
///   - Wired to embeddings, vision, and voice modules
///   - Contrastive alignment tracking
///
/// No data ever leaves the device. All fusion is local.
///
/// Inspired by: CLIP (OpenAI), ImageBind (Meta), Flamingo. All code is original.
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

/// Q16 square root via Newton-Raphson
fn q16_sqrt(x: i32) -> i32 {
    if x <= 0 {
        return 0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 16 {
        let div = q16_div(x, guess);
        guess = (guess + div) / 2;
        i += 1;
    }
    guess
}

/// Q16 approximate exp(x) for small x via Taylor series
/// exp(x) ~ 1 + x + x^2/2 + x^3/6
fn q16_exp_approx(x: i32) -> i32 {
    let x2 = q16_mul(x, x);
    let x3 = q16_mul(x2, x);
    Q16_ONE + x + q16_div(x2, q16_from_int(2)) + q16_div(x3, q16_from_int(6))
}

// ---------------------------------------------------------------------------
// Modality types and feature vectors
// ---------------------------------------------------------------------------

/// Unified embedding dimension
const UNIFIED_DIM: usize = 128;

/// Maximum features per modality
const MAX_FEATURES: usize = 256;

/// Maximum cross-modal attention heads
const MAX_ATTENTION_HEADS: usize = 4;

/// Maximum fused entries stored
const MAX_FUSED_ENTRIES: usize = 64;

/// Maximum alignment pairs tracked
const MAX_ALIGNMENTS: usize = 128;

/// The modality of an input signal
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    Text,
    Image,
    Audio,
    Video,  // future: treat as image + audio
    Sensor, // future: IMU, GPS, etc.
}

/// A feature vector from a single modality
pub struct ModalFeature {
    pub modality: Modality,
    pub vector: Vec<i32>, // Q16 values
    pub label: String,
    pub confidence: i32, // Q16
    pub timestamp: u64,
}

impl ModalFeature {
    /// Compute L2 norm of the feature vector (Q16)
    pub fn norm(&self) -> i32 {
        let sum: i64 = self
            .vector
            .iter()
            .map(|v| ((*v as i64) * (*v as i64)) >> 16)
            .sum();
        q16_sqrt(sum as i32)
    }

    /// Normalize the feature vector to unit length
    pub fn normalize(&mut self) {
        let n = self.norm();
        if n == 0 {
            return;
        }
        for v in &mut self.vector {
            *v = q16_div(*v, n);
        }
    }

    /// Dot product with another feature vector (Q16)
    pub fn dot(&self, other: &ModalFeature) -> i32 {
        let len = self.vector.len().min(other.vector.len());
        let mut sum: i64 = 0;
        let mut i = 0;
        while i < len {
            sum += ((self.vector[i] as i64) * (other.vector[i] as i64)) >> 16;
            i += 1;
        }
        sum as i32
    }

    /// Cosine similarity with another feature vector (Q16)
    pub fn cosine_similarity(&self, other: &ModalFeature) -> i32 {
        let dot = self.dot(other);
        let na = self.norm();
        let nb = other.norm();
        let denom = q16_mul(na, nb);
        if denom == 0 {
            return 0;
        }
        q16_div(dot, denom)
    }
}

// ---------------------------------------------------------------------------
// Cross-modal attention
// ---------------------------------------------------------------------------

/// A single attention head for cross-modal alignment
pub struct AttentionHead {
    pub query_modality: Modality,
    pub key_modality: Modality,
    pub weights: Vec<i32>, // Q16 learned attention weights (dim x dim)
    pub dim: usize,
    pub temperature: i32, // Q16 softmax temperature
}

impl AttentionHead {
    /// Create a new attention head with identity-like initialization
    fn new(query_mod: Modality, key_mod: Modality, dim: usize) -> Self {
        let diag_val = q16_div(Q16_ONE, q16_from_int(dim as i32));
        let mut weights = vec![0i32; dim * dim];
        let mut i = 0;
        while i < dim {
            weights[i * dim + i] = diag_val;
            i += 1;
        }
        AttentionHead {
            query_modality: query_mod,
            key_modality: key_mod,
            weights,
            dim,
            temperature: Q16_ONE,
        }
    }

    /// Compute attention score between a query vector and a key vector (Q16)
    /// score = (q^T * W * k) / temperature
    pub fn score(&self, query: &[i32], key: &[i32]) -> i32 {
        let dim = self.dim.min(query.len()).min(key.len());
        // Compute W * k first
        let mut wk = vec![0i32; dim];
        for row in 0..dim {
            let mut sum: i64 = 0;
            for col in 0..dim {
                let w = self.weights[row * self.dim + col];
                sum += ((w as i64) * (key[col] as i64)) >> 16;
            }
            wk[row] = sum as i32;
        }
        // Compute q^T * (W * k)
        let mut dot: i64 = 0;
        for i in 0..dim {
            dot += ((query[i] as i64) * (wk[i] as i64)) >> 16;
        }
        let raw = dot as i32;
        q16_div(raw, self.temperature)
    }
}

/// Softmax over a vector of Q16 scores (approximate, in-place)
fn q16_softmax(scores: &mut [i32]) {
    if scores.is_empty() {
        return;
    }
    let max_val = scores.iter().copied().max().unwrap_or(0);
    let mut exps = Vec::with_capacity(scores.len());
    let mut sum: i64 = 0;
    for s in scores.iter() {
        let shifted = *s - max_val;
        let clamped = if shifted < -(q16_from_int(4)) {
            -(q16_from_int(4))
        } else {
            shifted
        };
        let e = q16_exp_approx(clamped);
        exps.push(e);
        sum += e as i64;
    }
    if sum == 0 {
        sum = 1;
    }
    for (i, s) in scores.iter_mut().enumerate() {
        *s = (((exps[i] as i64) << 16) / sum) as i32;
    }
}

// ---------------------------------------------------------------------------
// Modal alignment (contrastive learning)
// ---------------------------------------------------------------------------

/// A pair of aligned features from different modalities
pub struct AlignmentPair {
    pub modality_a: Modality,
    pub modality_b: Modality,
    pub feature_a_label: String,
    pub feature_b_label: String,
    pub similarity: i32, // Q16 learned alignment score
    pub positive: bool,  // true = should be aligned, false = contrastive negative
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Unified fused embedding
// ---------------------------------------------------------------------------

/// A fused multimodal embedding combining information from all available modalities
pub struct FusedEmbedding {
    pub vector: Vec<i32>, // Q16 unified vector
    pub contributing_modalities: Vec<Modality>,
    pub modality_weights: Vec<(Modality, i32)>, // Q16 weight per modality
    pub confidence: i32,                        // Q16 overall confidence
    pub label: String,
    pub timestamp: u64,
}

impl FusedEmbedding {
    /// Similarity with another fused embedding
    pub fn similarity(&self, other: &FusedEmbedding) -> i32 {
        let len = self.vector.len().min(other.vector.len());
        if len == 0 {
            return 0;
        }
        let mut dot: i64 = 0;
        let mut norm_a: i64 = 0;
        let mut norm_b: i64 = 0;
        let mut i = 0;
        while i < len {
            dot += ((self.vector[i] as i64) * (other.vector[i] as i64)) >> 16;
            norm_a += ((self.vector[i] as i64) * (self.vector[i] as i64)) >> 16;
            norm_b += ((other.vector[i] as i64) * (other.vector[i] as i64)) >> 16;
            i += 1;
        }
        let na = q16_sqrt(norm_a as i32);
        let nb = q16_sqrt(norm_b as i32);
        let denom = q16_mul(na, nb);
        if denom == 0 {
            return 0;
        }
        q16_div(dot as i32, denom)
    }
}

// ---------------------------------------------------------------------------
// Cross-modal index entry (for search)
// ---------------------------------------------------------------------------

/// An entry in the cross-modal search index, linking a label+modality to a Q16 vector
struct IndexEntry {
    label: String,
    modality: Modality,
    vector: Vec<i32>, // Q16
}

// ---------------------------------------------------------------------------
// Fusion strategy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FusionStrategy {
    WeightedAverage, // Simple weighted combination
    Attention,       // Cross-modal attention fusion
    Concatenate,     // Concatenate and project
    Gated,           // Gating mechanism to select dominant modality
}

// ---------------------------------------------------------------------------
// Multimodal fusion engine
// ---------------------------------------------------------------------------

/// The multimodal fusion engine
pub struct MultimodalEngine {
    pub attention_heads: Vec<AttentionHead>,
    pub fused_entries: Vec<FusedEmbedding>,
    pub alignments: Vec<AlignmentPair>,
    pub strategy: FusionStrategy,
    pub unified_dim: usize,
    pub enabled: bool,
    pub total_fusions: u64,
    pub modality_default_weights: Vec<(Modality, i32)>, // Q16 default weight per modality
    /// Cross-modal search index: stores embeddings from different modalities
    /// so we can search across them (e.g., find images matching a text query)
    search_index: Vec<IndexEntry>,
}

impl MultimodalEngine {
    const fn new() -> Self {
        MultimodalEngine {
            attention_heads: Vec::new(),
            fused_entries: Vec::new(),
            alignments: Vec::new(),
            strategy: FusionStrategy::WeightedAverage,
            unified_dim: UNIFIED_DIM,
            enabled: true,
            total_fusions: 0,
            modality_default_weights: Vec::new(),
            search_index: Vec::new(),
        }
    }

    // ----- Fusion methods -----

    /// Fuse multiple modal features into a unified embedding
    pub fn fuse(&mut self, features: &[ModalFeature]) -> FusedEmbedding {
        self.total_fusions = self.total_fusions.saturating_add(1);
        match self.strategy {
            FusionStrategy::WeightedAverage => self.fuse_weighted(features),
            FusionStrategy::Attention => self.fuse_attention(features),
            FusionStrategy::Concatenate => self.fuse_concat(features),
            FusionStrategy::Gated => self.fuse_gated(features),
        }
    }

    /// Weighted average fusion
    fn fuse_weighted(&self, features: &[ModalFeature]) -> FusedEmbedding {
        let dim = self.unified_dim;
        let mut vector = vec![0i32; dim];
        let mut mods = Vec::new();
        let mut mod_weights = Vec::new();
        let mut total_weight: i64 = 0;

        for feat in features {
            let w = self.get_modality_weight(feat.modality);
            let feat_w = q16_mul(w, feat.confidence);
            total_weight += feat_w as i64;
            mods.push(feat.modality);
            mod_weights.push((feat.modality, feat_w));

            let flen = feat.vector.len().min(dim);
            for i in 0..flen {
                vector[i] = vector[i].saturating_add(q16_mul(feat.vector[i], feat_w));
            }
        }

        if total_weight > 0 {
            for v in &mut vector {
                *v = (((*v as i64) << 16) / total_weight.max(1)) as i32;
            }
        }

        let confidence = if features.is_empty() {
            0
        } else {
            let sum: i64 = features.iter().map(|f| f.confidence as i64).sum();
            (sum / (features.len() as i64)) as i32
        };

        let now = crate::time::clock::unix_time();
        FusedEmbedding {
            vector,
            contributing_modalities: mods,
            modality_weights: mod_weights,
            confidence,
            label: String::from("weighted_avg"),
            timestamp: now,
        }
    }

    /// Attention-based fusion using cross-modal attention heads
    fn fuse_attention(&self, features: &[ModalFeature]) -> FusedEmbedding {
        let dim = self.unified_dim;
        let mut vector = vec![0i32; dim];
        let mut mods = Vec::new();
        let mut mod_weights = Vec::new();

        if features.is_empty() {
            let now = crate::time::clock::unix_time();
            return FusedEmbedding {
                vector,
                contributing_modalities: mods,
                modality_weights: mod_weights,
                confidence: 0,
                label: String::from("attention_empty"),
                timestamp: now,
            };
        }

        for (qi, query_feat) in features.iter().enumerate() {
            let mut attn_scores: Vec<i32> = Vec::new();
            for (ki, key_feat) in features.iter().enumerate() {
                if qi == ki {
                    attn_scores.push(Q16_ONE);
                    continue;
                }
                let score = self
                    .attention_heads
                    .iter()
                    .find(|h| {
                        h.query_modality == query_feat.modality
                            && h.key_modality == key_feat.modality
                    })
                    .map(|h| h.score(&query_feat.vector, &key_feat.vector))
                    .unwrap_or(0);
                attn_scores.push(score);
            }

            q16_softmax(&mut attn_scores);

            let flen = query_feat.vector.len().min(dim);
            for (ki, key_feat) in features.iter().enumerate() {
                let w = attn_scores[ki];
                let klen = key_feat.vector.len().min(dim);
                let limit = flen.min(klen);
                for i in 0..limit {
                    vector[i] = vector[i].saturating_add(q16_mul(key_feat.vector[i], w));
                }
            }

            mods.push(query_feat.modality);
        }

        let nf = features.len() as i32;
        if nf > 1 {
            for v in &mut vector {
                *v = q16_div(*v, q16_from_int(nf));
            }
        }

        let avg_conf = {
            let sum: i64 = features.iter().map(|f| f.confidence as i64).sum();
            (sum / (features.len() as i64)) as i32
        };

        for feat in features {
            mod_weights.push((feat.modality, feat.confidence));
        }

        let now = crate::time::clock::unix_time();
        FusedEmbedding {
            vector,
            contributing_modalities: mods,
            modality_weights: mod_weights,
            confidence: avg_conf,
            label: String::from("attention_fused"),
            timestamp: now,
        }
    }

    /// Concatenation-based fusion: concatenate then project to unified dim
    fn fuse_concat(&self, features: &[ModalFeature]) -> FusedEmbedding {
        let dim = self.unified_dim;
        let mut vector = vec![0i32; dim];
        let mut mods = Vec::new();
        let mut mod_weights = Vec::new();

        let mut concat = Vec::new();
        for feat in features {
            concat.extend_from_slice(&feat.vector);
            mods.push(feat.modality);
            mod_weights.push((feat.modality, feat.confidence));
        }

        if !concat.is_empty() {
            for out_i in 0..dim {
                let mut sum: i64 = 0;
                let stride = concat.len() / dim;
                let stride = if stride == 0 { 1 } else { stride };
                let start = (out_i * stride) % concat.len();
                let mut j = 0;
                while j < stride && (start + j) < concat.len() {
                    sum += concat[start + j] as i64;
                    j += 1;
                }
                vector[out_i] = if stride > 0 {
                    (sum / (stride as i64)) as i32
                } else {
                    0
                };
            }
        }

        let confidence = if features.is_empty() {
            0
        } else {
            let sum: i64 = features.iter().map(|f| f.confidence as i64).sum();
            (sum / (features.len() as i64)) as i32
        };

        let now = crate::time::clock::unix_time();
        FusedEmbedding {
            vector,
            contributing_modalities: mods,
            modality_weights: mod_weights,
            confidence,
            label: String::from("concat_projected"),
            timestamp: now,
        }
    }

    /// Gated fusion: select the dominant modality with soft gating
    fn fuse_gated(&self, features: &[ModalFeature]) -> FusedEmbedding {
        let dim = self.unified_dim;
        let mut vector = vec![0i32; dim];
        let mut mods = Vec::new();
        let mut mod_weights = Vec::new();

        if features.is_empty() {
            let now = crate::time::clock::unix_time();
            return FusedEmbedding {
                vector,
                contributing_modalities: mods,
                modality_weights: mod_weights,
                confidence: 0,
                label: String::from("gated_empty"),
                timestamp: now,
            };
        }

        let mut gate_scores: Vec<i32> = features
            .iter()
            .map(|f| q16_mul(f.confidence, self.get_modality_weight(f.modality)))
            .collect();

        q16_softmax(&mut gate_scores);

        for (fi, feat) in features.iter().enumerate() {
            let g = gate_scores[fi];
            let flen = feat.vector.len().min(dim);
            for i in 0..flen {
                vector[i] = vector[i].saturating_add(q16_mul(feat.vector[i], g));
            }
            mods.push(feat.modality);
            mod_weights.push((feat.modality, g));
        }

        let max_gate = gate_scores.iter().copied().max().unwrap_or(0);
        let confidence = features
            .iter()
            .zip(gate_scores.iter())
            .max_by_key(|(_, g)| **g)
            .map(|(f, _)| f.confidence)
            .unwrap_or(0);

        let now = crate::time::clock::unix_time();
        FusedEmbedding {
            vector,
            contributing_modalities: mods,
            modality_weights: mod_weights,
            confidence: q16_mul(confidence, max_gate + Q16_ONE / 2),
            label: String::from("gated_fused"),
            timestamp: now,
        }
    }

    /// Get the default weight for a modality
    fn get_modality_weight(&self, modality: Modality) -> i32 {
        self.modality_default_weights
            .iter()
            .find(|(m, _)| *m == modality)
            .map(|(_, w)| *w)
            .unwrap_or(Q16_ONE)
    }

    // ----- Cross-modal search -----

    /// Index a feature for cross-modal search. The feature's vector and label
    /// are stored so it can be retrieved by queries from any modality.
    pub fn index_feature(&mut self, feature: &ModalFeature) {
        // Avoid duplicates by label+modality
        let exists = self
            .search_index
            .iter()
            .any(|e| e.label == feature.label && e.modality == feature.modality);
        if exists {
            return;
        }

        // Project to unified dim if needed
        let projected = project_to_dim(&feature.vector, self.unified_dim);
        self.search_index.push(IndexEntry {
            label: feature.label.clone(),
            modality: feature.modality,
            vector: projected,
        });

        // Cap index size
        if self.search_index.len() > MAX_FEATURES {
            self.search_index.remove(0);
        }
    }

    /// Create a text feature from a text string using the embeddings module,
    /// then project into Q16 unified space
    pub fn embed_text(&self, text: &str) -> ModalFeature {
        let embedding = super::embeddings::embed(text);
        let q16_vector = f32_vec_to_q16(&embedding.vector, self.unified_dim);
        let now = crate::time::clock::unix_time();
        ModalFeature {
            modality: Modality::Text,
            vector: q16_vector,
            label: String::from(text),
            confidence: Q16_ONE * 9 / 10, // 0.9 — text embeddings are generally reliable
            timestamp: now,
        }
    }

    /// Search the index for entries matching a text query.
    /// Returns (label, modality, similarity_score) sorted by similarity descending.
    pub fn search_by_text(
        &self,
        query_text: &str,
        target_modality: Option<Modality>,
        top_k: usize,
    ) -> Vec<(String, Modality, i32)> {
        let query_emb = super::embeddings::embed(query_text);
        let query_vec = f32_vec_to_q16(&query_emb.vector, self.unified_dim);
        self.search_index_by_vector(&query_vec, target_modality, top_k)
    }

    /// Search the index for entries matching a given feature vector.
    pub fn search_by_feature(
        &self,
        feature: &ModalFeature,
        target_modality: Option<Modality>,
        top_k: usize,
    ) -> Vec<(String, Modality, i32)> {
        let projected = project_to_dim(&feature.vector, self.unified_dim);
        self.search_index_by_vector(&projected, target_modality, top_k)
    }

    /// Core search: find indexed entries most similar to a query vector
    fn search_index_by_vector(
        &self,
        query_vec: &[i32],
        target_modality: Option<Modality>,
        top_k: usize,
    ) -> Vec<(String, Modality, i32)> {
        let mut results: Vec<(String, Modality, i32)> = Vec::new();

        for entry in &self.search_index {
            // Filter by target modality if specified
            if let Some(target) = target_modality {
                if entry.modality != target {
                    continue;
                }
            }
            let sim = cosine_similarity_q16(query_vec, &entry.vector);
            results.push((entry.label.clone(), entry.modality, sim));
        }

        results.sort_by(|a, b| b.2.cmp(&a.2));
        results.truncate(top_k);
        results
    }

    // ----- Alignment -----

    /// Record a contrastive alignment pair
    pub fn record_alignment(&mut self, a: &ModalFeature, b: &ModalFeature, positive: bool) {
        let sim = a.cosine_similarity(b);
        let now = crate::time::clock::unix_time();
        self.alignments.push(AlignmentPair {
            modality_a: a.modality,
            modality_b: b.modality,
            feature_a_label: a.label.clone(),
            feature_b_label: b.label.clone(),
            similarity: sim,
            positive,
            timestamp: now,
        });
        if self.alignments.len() > MAX_ALIGNMENTS {
            self.alignments.remove(0);
        }
    }

    // ----- Fused embedding storage -----

    /// Store a fused embedding for later retrieval
    pub fn store_fused(&mut self, fused: FusedEmbedding) {
        self.fused_entries.push(fused);
        if self.fused_entries.len() > MAX_FUSED_ENTRIES {
            self.fused_entries.remove(0);
        }
    }

    /// Search stored fused embeddings by similarity to a query
    pub fn search_fused(&self, query: &FusedEmbedding, top_k: usize) -> Vec<(usize, i32)> {
        let mut results: Vec<(usize, i32)> = self
            .fused_entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, query.similarity(e)))
            .collect();
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.truncate(top_k);
        results
    }

    /// Set the fusion strategy
    pub fn set_strategy(&mut self, strategy: FusionStrategy) {
        self.strategy = strategy;
    }

    /// Get statistics
    pub fn stats(&self) -> (u64, usize, usize, usize, usize) {
        (
            self.total_fusions,
            self.fused_entries.len(),
            self.alignments.len(),
            self.attention_heads.len(),
            self.search_index.len(),
        )
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Convert an f32 embedding vector to Q16 i32, projecting to target dimension
fn f32_vec_to_q16(vec: &[f32], target_dim: usize) -> Vec<i32> {
    let mut result = alloc::vec![0i32; target_dim];
    if vec.is_empty() {
        return result;
    }

    if vec.len() <= target_dim {
        // Pad with zeros
        for (i, &v) in vec.iter().enumerate() {
            result[i] = (v * 65536.0) as i32;
        }
    } else {
        // Average pooling to reduce dimension
        let stride = vec.len() / target_dim;
        for i in 0..target_dim {
            let start = i * stride;
            let end = (start + stride).min(vec.len());
            let mut sum = 0.0f32;
            let mut count = 0;
            for j in start..end {
                sum += vec[j];
                count += 1;
            }
            if count > 0 {
                result[i] = ((sum / count as f32) * 65536.0) as i32;
            }
        }
    }
    result
}

/// Project a Q16 vector to a target dimension (truncate or zero-pad)
fn project_to_dim(vec: &[i32], target_dim: usize) -> Vec<i32> {
    let mut result = alloc::vec![0i32; target_dim];
    let copy_len = vec.len().min(target_dim);
    result[..copy_len].copy_from_slice(&vec[..copy_len]);
    result
}

/// Cosine similarity between two Q16 vectors
fn cosine_similarity_q16(a: &[i32], b: &[i32]) -> i32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0;
    }
    let mut dot: i64 = 0;
    let mut norm_a: i64 = 0;
    let mut norm_b: i64 = 0;
    for i in 0..len {
        dot += ((a[i] as i64) * (b[i] as i64)) >> 16;
        norm_a += ((a[i] as i64) * (a[i] as i64)) >> 16;
        norm_b += ((b[i] as i64) * (b[i] as i64)) >> 16;
    }
    let na = q16_sqrt(norm_a as i32);
    let nb = q16_sqrt(norm_b as i32);
    let denom = q16_mul(na, nb);
    if denom == 0 {
        return 0;
    }
    q16_div(dot as i32, denom)
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static MULTIMODAL: Mutex<Option<MultimodalEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = MultimodalEngine::new();

    // Set default modality weights (text is primary, others supplement)
    engine.modality_default_weights = vec![
        (Modality::Text, Q16_ONE),
        (Modality::Image, Q16_ONE * 4 / 5), // 0.8
        (Modality::Audio, Q16_ONE * 3 / 4), // 0.75
        (Modality::Video, Q16_ONE * 4 / 5), // 0.8
        (Modality::Sensor, Q16_ONE / 2),    // 0.5
    ];

    // Initialize cross-modal attention heads
    let dim = UNIFIED_DIM;
    engine
        .attention_heads
        .push(AttentionHead::new(Modality::Text, Modality::Image, dim));
    engine
        .attention_heads
        .push(AttentionHead::new(Modality::Text, Modality::Audio, dim));
    engine
        .attention_heads
        .push(AttentionHead::new(Modality::Image, Modality::Audio, dim));
    engine
        .attention_heads
        .push(AttentionHead::new(Modality::Image, Modality::Text, dim));

    *MULTIMODAL.lock() = Some(engine);
    serial_println!("    [multimodal] Multimodal AI fusion engine initialized (dim={}, 4 attention heads, cross-modal search)", UNIFIED_DIM);
}

/// Fuse multiple modal features into a unified embedding
pub fn fuse(features: &[ModalFeature]) -> Option<FusedEmbedding> {
    MULTIMODAL.lock().as_mut().map(|e| e.fuse(features))
}

/// Store a fused embedding
pub fn store(fused: FusedEmbedding) {
    if let Some(engine) = MULTIMODAL.lock().as_mut() {
        engine.store_fused(fused);
    }
}

/// Search stored fused embeddings by similarity
pub fn search(query: &FusedEmbedding, top_k: usize) -> Vec<(usize, i32)> {
    MULTIMODAL
        .lock()
        .as_ref()
        .map(|e| e.search_fused(query, top_k))
        .unwrap_or_default()
}

/// Set the fusion strategy
pub fn set_strategy(strategy: FusionStrategy) {
    if let Some(engine) = MULTIMODAL.lock().as_mut() {
        engine.set_strategy(strategy);
    }
}

/// Index a feature for cross-modal search
pub fn index_feature(feature: &ModalFeature) {
    if let Some(engine) = MULTIMODAL.lock().as_mut() {
        engine.index_feature(feature);
    }
}

/// Create a text embedding feature using the embeddings module
pub fn embed_text(text: &str) -> Option<ModalFeature> {
    MULTIMODAL.lock().as_ref().map(|e| e.embed_text(text))
}

/// Cross-modal search: find entries in the index matching a text query.
/// Optionally filter by target modality (e.g., Modality::Image to find images).
pub fn search_by_text(
    query: &str,
    target_modality: Option<Modality>,
    top_k: usize,
) -> Vec<(String, Modality, i32)> {
    MULTIMODAL
        .lock()
        .as_ref()
        .map(|e| e.search_by_text(query, target_modality, top_k))
        .unwrap_or_default()
}

/// Cross-modal search: find entries matching a feature vector
pub fn search_by_feature(
    feature: &ModalFeature,
    target_modality: Option<Modality>,
    top_k: usize,
) -> Vec<(String, Modality, i32)> {
    MULTIMODAL
        .lock()
        .as_ref()
        .map(|e| e.search_by_feature(feature, target_modality, top_k))
        .unwrap_or_default()
}

/// Get statistics: (total_fusions, stored_entries, alignments, attention_heads, indexed_features)
pub fn stats() -> (u64, usize, usize, usize, usize) {
    MULTIMODAL
        .lock()
        .as_ref()
        .map(|e| e.stats())
        .unwrap_or((0, 0, 0, 0, 0))
}

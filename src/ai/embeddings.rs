use crate::sync::Mutex;
/// Text embeddings — vector representations for semantic search
///
/// Converts text into dense vectors for:
///   - Semantic file search
///   - Similar document finding
///   - AI memory retrieval
///   - Smart autocomplete
///
/// Uses character n-gram hashing for semantically meaningful embeddings:
/// similar text produces similar vectors because they share n-grams.
use crate::{serial_print, serial_println};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

static EMBED_ENGINE: Mutex<Option<EmbeddingEngine>> = Mutex::new(None);

const EMBEDDING_DIM: usize = 384; // MiniLM-style dimensionality

/// An embedding vector
#[derive(Debug, Clone)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub text: String,
    pub norm: f32,
}

/// Software sqrt for no_std (Newton-Raphson)
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 15 {
        guess = (guess + x / guess) * 0.5;
        i += 1;
    }
    guess
}

impl Embedding {
    pub fn new(vector: Vec<f32>, text: String) -> Self {
        let norm = sqrt_f32(vector.iter().map(|x| x * x).sum::<f32>());
        Embedding { vector, text, norm }
    }

    /// Cosine similarity between two embeddings
    pub fn cosine_similarity(&self, other: &Embedding) -> f32 {
        if self.norm == 0.0 || other.norm == 0.0 {
            return 0.0;
        }

        let dot: f32 = self
            .vector
            .iter()
            .zip(other.vector.iter())
            .map(|(a, b)| a * b)
            .sum();

        dot / (self.norm * other.norm)
    }
}

// ---------------------------------------------------------------------------
// N-gram hashing helpers
// ---------------------------------------------------------------------------

/// FNV-1a hash of a byte slice
fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hash an n-gram to a dimension index and a signed weight.
/// The sign is derived from a secondary hash bit, giving the
/// embedding a zero-mean property for better discrimination.
fn ngram_to_dim_weight(ngram: &[u8]) -> (usize, f32) {
    let h = fnv1a_hash(ngram);
    let dim = (h as usize) % EMBEDDING_DIM;
    let weight = if (h >> 63) == 0 { 1.0 } else { -1.0 };
    (dim, weight)
}

/// Normalize a string for n-gram extraction: lowercase ASCII,
/// collapse whitespace, trim.
fn normalize_for_ngrams(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut last_was_space = true;
    for &b in text.as_bytes() {
        let ch = match b {
            b'A'..=b'Z' => b + 32,
            b'\t' | b'\n' | b'\r' => b' ',
            _ => b,
        };
        if ch == b' ' {
            if !last_was_space {
                out.push(ch);
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    if out.last() == Some(&b' ') {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// VectorStore for efficient retrieval
// ---------------------------------------------------------------------------

/// Entry in the vector store
#[derive(Debug, Clone)]
pub struct VectorEntry {
    pub id: u32,
    pub embedding: Embedding,
    pub metadata: String,
}

/// A persistent store of embeddings with metadata for retrieval
pub struct VectorStore {
    entries: Vec<VectorEntry>,
    next_id: u32,
}

impl VectorStore {
    pub fn new() -> Self {
        VectorStore {
            entries: Vec::new(),
            next_id: 1,
        }
    }

    /// Insert a new entry. Returns the assigned id.
    pub fn insert(&mut self, embedding: Embedding, metadata: String) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.entries.push(VectorEntry {
            id,
            embedding,
            metadata,
        });
        id
    }

    /// Remove an entry by id. Returns true if found and removed.
    pub fn remove(&mut self, id: u32) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        self.entries.len() < before
    }

    /// Search the store for entries most similar to `query`.
    /// Returns up to `top_k` results as (id, similarity_score, metadata).
    pub fn search(&self, query: &Embedding, top_k: usize) -> Vec<(u32, f32, &str)> {
        let mut scores: Vec<(u32, f32, &str)> = self
            .entries
            .iter()
            .map(|e| {
                (
                    e.id,
                    query.cosine_similarity(&e.embedding),
                    e.metadata.as_str(),
                )
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores
    }

    /// Number of stored entries
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store is empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get an entry by id
    pub fn get(&self, id: u32) -> Option<&VectorEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

// ---------------------------------------------------------------------------
// Embedding engine
// ---------------------------------------------------------------------------

/// Embedding model engine
pub struct EmbeddingEngine {
    pub dim: usize,
    pub model_loaded: bool,
}

impl EmbeddingEngine {
    pub fn new() -> Self {
        EmbeddingEngine {
            dim: EMBEDDING_DIM,
            model_loaded: false,
        }
    }

    /// Embed a text string using character n-gram features.
    ///
    /// Extracts all 2-grams and 3-grams from the normalized text,
    /// hashes each to a dimension index, and accumulates signed weights.
    /// Word-level unigrams are added with higher weight for
    /// longer-range semantic signal.  The result is L2-normalized.
    pub fn embed(&self, text: &str) -> Embedding {
        let mut vector = vec![0.0f32; self.dim];
        let normalized = normalize_for_ngrams(text);
        let len = normalized.len();

        if len == 0 {
            return Embedding::new(vector, String::from(text));
        }

        // 2-grams
        if len >= 2 {
            for i in 0..len - 1 {
                let ngram = &normalized[i..i + 2];
                let (dim, weight) = ngram_to_dim_weight(ngram);
                vector[dim] += weight;
            }
        }

        // 3-grams (weighted more heavily for better discrimination)
        if len >= 3 {
            for i in 0..len - 2 {
                let ngram = &normalized[i..i + 3];
                let (dim, weight) = ngram_to_dim_weight(ngram);
                vector[dim] += weight * 1.5;
            }
        }

        // Word-level unigrams for longer-range signal
        let mut word_start = 0;
        for i in 0..=len {
            let is_boundary = i == len || normalized[i] == b' ';
            if is_boundary && i > word_start {
                let word = &normalized[word_start..i];
                if word.len() >= 2 {
                    let (dim, weight) = ngram_to_dim_weight(word);
                    vector[dim] += weight * 2.0;
                }
                word_start = i + 1;
            }
        }

        // L2-normalize so cosine similarity works correctly
        let norm = sqrt_f32(vector.iter().map(|x| x * x).sum::<f32>());
        if norm > 0.0 {
            for v in vector.iter_mut() {
                *v /= norm;
            }
        }

        Embedding::new(vector, String::from(text))
    }

    /// Embed multiple texts at once
    pub fn embed_batch(&self, texts: &[&str]) -> Vec<Embedding> {
        let mut results = Vec::with_capacity(texts.len());
        for &text in texts {
            results.push(self.embed(text));
        }
        results
    }

    /// Find the most similar embeddings from a collection
    pub fn search(
        &self,
        query: &Embedding,
        corpus: &[Embedding],
        top_k: usize,
    ) -> Vec<(usize, f32)> {
        let mut scores: Vec<(usize, f32)> = corpus
            .iter()
            .enumerate()
            .map(|(i, emb)| (i, query.cosine_similarity(emb)))
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        scores.truncate(top_k);
        scores
    }
}

pub fn init() {
    *EMBED_ENGINE.lock() = Some(EmbeddingEngine::new());
    serial_println!(
        "    [embeddings] Embedding engine ready (dim={}, n-gram hashing)",
        EMBEDDING_DIM
    );
}

/// Embed a text string
pub fn embed(text: &str) -> Embedding {
    EMBED_ENGINE
        .lock()
        .as_ref()
        .map(|e| e.embed(text))
        .unwrap_or_else(|| Embedding::new(Vec::new(), String::from(text)))
}

/// Embed multiple texts at once
pub fn embed_batch(texts: &[&str]) -> Vec<Embedding> {
    EMBED_ENGINE
        .lock()
        .as_ref()
        .map(|e| e.embed_batch(texts))
        .unwrap_or_else(|| {
            texts
                .iter()
                .map(|t| Embedding::new(Vec::new(), String::from(*t)))
                .collect()
        })
}

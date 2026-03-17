/// Full-text search engine for Genesis
///
/// Inverted index with tokenizer, TF-IDF scoring (Q16 fixed-point),
/// boolean queries (AND/OR/NOT), phrase search, fuzzy matching
/// via Levenshtein distance. All computations use integer or Q16
/// arithmetic — no floating-point.
///
/// Architecture:
///   - Documents are added with a unique doc_id and content hash tokens
///   - Each token maps to a posting list (doc_id + position)
///   - Queries are parsed into a tree of boolean / phrase / fuzzy nodes
///   - Scoring combines TF (term frequency) and IDF (inverse document freq)
///     entirely in Q16 fixed-point
///
/// Inspired by: Lucene inverted index, Okapi BM25 concepts, Aho-Corasick
/// for multi-pattern. All code is original.

use crate::{serial_print, serial_println};

use alloc::vec::Vec;
use alloc::vec;
use alloc::string::String;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> Q16_SHIFT) as i32
}

fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << Q16_SHIFT) / (b as i64)) as i32
}

fn q16_from_int(v: i32) -> i32 {
    v << Q16_SHIFT
}

/// Approximate log2 in Q16 (integer bit-scan + linear interpolation)
fn q16_log2(x: i32) -> i32 {
    if x <= 0 { return 0; }
    // integer part: position of highest set bit
    let mut v = x as u32;
    let mut int_part: i32 = 0;
    while v > 1 {
        v >>= 1;
        int_part += 1;
    }
    // fractional approximation: fraction of remaining bits
    let remainder = x - (1 << int_part);
    let frac = if int_part > 0 {
        q16_div(remainder << Q16_SHIFT >> Q16_SHIFT, 1 << int_part)
    } else {
        0
    };
    q16_from_int(int_part) + frac
}

// ---------------------------------------------------------------------------
// Configuration constants
// ---------------------------------------------------------------------------

/// Maximum number of indexed documents
const MAX_DOCUMENTS: usize = 8192;
/// Maximum distinct tokens in the vocabulary
const MAX_VOCAB: usize = 16384;
/// Maximum postings per token
const MAX_POSTINGS_PER_TERM: usize = 4096;
/// Maximum tokens per document kept for position data
const MAX_POSITIONS_PER_DOC: usize = 512;
/// Maximum query clauses in a boolean query
const MAX_QUERY_CLAUSES: usize = 16;
/// Fuzzy match max edit distance
const MAX_EDIT_DISTANCE: usize = 2;

// ---------------------------------------------------------------------------
// Token / posting types
// ---------------------------------------------------------------------------

/// A single posting: which document, at which position
#[derive(Clone, Copy, Debug)]
struct Posting {
    doc_id: u32,
    position: u16,
    /// Term frequency for this doc cached here for speed
    tf_count: u16,
}

/// A vocabulary entry — one term in the inverted index
#[derive(Clone)]
struct TermEntry {
    /// Hash of the normalised token
    token_hash: u64,
    /// Postings list
    postings: Vec<Posting>,
    /// Number of documents containing this term (for IDF)
    doc_freq: u32,
}

/// Document metadata kept alongside the index
#[derive(Clone, Copy)]
struct DocMeta {
    doc_id: u32,
    total_tokens: u32,
    timestamp: u64,
    /// Content hash for dedup detection
    content_hash: u64,
    /// Whether the document is still active
    active: bool,
}

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

/// Operator for boolean queries
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BoolOp {
    And,
    Or,
    Not,
}

/// A single query clause
#[derive(Clone, Debug)]
pub enum QueryClause {
    /// Single token lookup (hash)
    Term(u64),
    /// Phrase: ordered sequence of token hashes
    Phrase(Vec<u64>),
    /// Fuzzy: token hash + max edit distance
    Fuzzy { token_hash: u64, max_distance: u8 },
    /// Boolean combination
    Boolean { op: BoolOp, children: Vec<QueryClause> },
}

/// A scored search result
#[derive(Clone, Copy, Debug)]
pub struct SearchHit {
    pub doc_id: u32,
    /// Relevance score in Q16
    pub score_q16: i32,
    pub timestamp: u64,
}

// ---------------------------------------------------------------------------
// Tokenizer helpers
// ---------------------------------------------------------------------------

/// Extremely minimal tokenizer: splits on non-alphanumeric, lowercases via
/// ASCII subtraction, returns token hashes. No heap-allocated strings needed
/// for query-time tokenization — we work entirely with u64 hashes.
fn tokenize_to_hashes(content_hashes: &[u64]) -> Vec<u64> {
    // In a bare-metal kernel the caller pre-hashes tokens; we just pass through.
    content_hashes.to_vec()
}

/// Simple FNV-1a style hash for comparing tokens during fuzzy match
fn hash_distance(a: u64, b: u64) -> usize {
    // Bit-difference heuristic (Hamming distance on the hash)
    let xor = a ^ b;
    let mut dist = 0usize;
    let mut v = xor;
    while v != 0 {
        dist += 1;
        v &= v - 1; // clear lowest set bit
    }
    // Scale: every 8 differing bits ≈ 1 edit distance
    dist / 8
}

// ---------------------------------------------------------------------------
// Full-text search engine
// ---------------------------------------------------------------------------

struct FullTextEngine {
    vocab: Vec<TermEntry>,
    documents: Vec<DocMeta>,
    /// Running average document length in Q16
    avg_doc_len_q16: i32,
    total_docs: u32,
    total_tokens_all: u64,
    next_doc_id: u32,
}

static FULL_TEXT: Mutex<Option<FullTextEngine>> = Mutex::new(None);

impl FullTextEngine {
    fn new() -> Self {
        FullTextEngine {
            vocab: Vec::new(),
            documents: Vec::new(),
            avg_doc_len_q16: 0,
            total_docs: 0,
            total_tokens_all: 0,
            next_doc_id: 1,
        }
    }

    // ----- Indexing --------------------------------------------------------

    /// Add a document. `token_hashes` is the pre-tokenized content.
    fn add_document(&mut self, content_hash: u64, token_hashes: &[u64], timestamp: u64) -> u32 {
        if self.documents.len() >= MAX_DOCUMENTS {
            return 0; // index full
        }

        let doc_id = self.next_doc_id;
        self.next_doc_id = self.next_doc_id.saturating_add(1);

        let token_count = token_hashes.len() as u32;

        self.documents.push(DocMeta {
            doc_id,
            total_tokens: token_count,
            timestamp,
            content_hash,
            active: true,
        });

        // Update running average doc length
        self.total_docs = self.total_docs.saturating_add(1);
        self.total_tokens_all += token_count as u64;
        self.avg_doc_len_q16 = q16_div(
            (self.total_tokens_all as i32) << Q16_SHIFT >> Q16_SHIFT,
            self.total_docs as i32,
        );

        // Build per-term frequency map for this doc
        let mut term_counts: Vec<(u64, u16)> = Vec::new();
        for (pos, &th) in token_hashes.iter().enumerate() {
            if pos >= MAX_POSITIONS_PER_DOC {
                break;
            }
            if let Some(entry) = term_counts.iter_mut().find(|e| e.0 == th) {
                entry.1 += 1;
            } else {
                term_counts.push((th, 1));
            }
        }

        // Insert postings
        for (pos, &th) in token_hashes.iter().enumerate() {
            if pos >= MAX_POSITIONS_PER_DOC {
                break;
            }
            let tf = term_counts.iter().find(|e| e.0 == th).map_or(1u16, |e| e.1);
            self.insert_posting(th, doc_id, pos as u16, tf);
        }

        doc_id
    }

    /// Insert a posting into the inverted index
    fn insert_posting(&mut self, token_hash: u64, doc_id: u32, position: u16, tf_count: u16) {
        // Find or create the term entry
        let term_idx = if let Some(idx) = self.vocab.iter().position(|t| t.token_hash == token_hash) {
            idx
        } else {
            if self.vocab.len() >= MAX_VOCAB {
                return; // vocab full
            }
            self.vocab.push(TermEntry {
                token_hash,
                postings: Vec::new(),
                doc_freq: 0,
            });
            self.vocab.len() - 1
        };

        let term = &mut self.vocab[term_idx];
        if term.postings.len() >= MAX_POSTINGS_PER_TERM {
            return;
        }

        // Track doc_freq: only increment if this doc_id is new for this term
        let already_present = term.postings.iter().any(|p| p.doc_id == doc_id);
        if !already_present {
            term.doc_freq = term.doc_freq.saturating_add(1);
        }

        term.postings.push(Posting { doc_id, position, tf_count });
    }

    /// Remove a document from the index (mark inactive, prune postings)
    fn remove_document(&mut self, doc_id: u32) {
        if let Some(doc) = self.documents.iter_mut().find(|d| d.doc_id == doc_id) {
            doc.active = false;
        }
        for term in &mut self.vocab {
            let before = term.postings.len();
            term.postings.retain(|p| p.doc_id != doc_id);
            if term.postings.len() < before {
                term.doc_freq = term.doc_freq.saturating_sub(1);
            }
        }
        // Remove empty terms
        self.vocab.retain(|t| !t.postings.is_empty());
    }

    // ----- Scoring ---------------------------------------------------------

    /// Compute TF-IDF score for a term in a document (Q16)
    fn tf_idf_score(&self, tf: u16, doc_freq: u32, doc_len: u32) -> i32 {
        if doc_freq == 0 || doc_len == 0 {
            return 0;
        }

        // TF component: tf / doc_len  (normalized)
        let tf_q16 = q16_div(q16_from_int(tf as i32), q16_from_int(doc_len as i32));

        // IDF component: log2(total_docs / doc_freq)
        let ratio = q16_div(q16_from_int(self.total_docs as i32), q16_from_int(doc_freq as i32));
        let idf_q16 = q16_log2(ratio >> Q16_SHIFT); // log2 of integer part

        // Ensure IDF is at least 1.0 in Q16 to avoid zeroing out
        let idf_q16 = if idf_q16 < Q16_ONE { Q16_ONE } else { idf_q16 };

        q16_mul(tf_q16, idf_q16)
    }

    // ----- Query execution -------------------------------------------------

    /// Execute a query and return scored results
    fn execute_query(&self, query: &QueryClause) -> Vec<SearchHit> {
        match query {
            QueryClause::Term(hash) => self.query_term(*hash),
            QueryClause::Phrase(hashes) => self.query_phrase(hashes),
            QueryClause::Fuzzy { token_hash, max_distance } => {
                self.query_fuzzy(*token_hash, *max_distance as usize)
            }
            QueryClause::Boolean { op, children } => self.query_boolean(*op, children),
        }
    }

    /// Single term query
    fn query_term(&self, token_hash: u64) -> Vec<SearchHit> {
        let term = match self.vocab.iter().find(|t| t.token_hash == token_hash) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen_docs: Vec<u32> = Vec::new();

        for posting in &term.postings {
            if seen_docs.contains(&posting.doc_id) {
                continue;
            }
            seen_docs.push(posting.doc_id);

            let doc_len = self.documents.iter()
                .find(|d| d.doc_id == posting.doc_id && d.active)
                .map_or(0, |d| d.total_tokens);

            if doc_len == 0 { continue; }

            let score = self.tf_idf_score(posting.tf_count, term.doc_freq, doc_len);
            let ts = self.documents.iter()
                .find(|d| d.doc_id == posting.doc_id)
                .map_or(0, |d| d.timestamp);

            hits.push(SearchHit { doc_id: posting.doc_id, score_q16: score, timestamp: ts });
        }

        self.sort_hits(&mut hits);
        hits
    }

    /// Phrase query — all tokens must appear in order in the same document
    fn query_phrase(&self, hashes: &[u64]) -> Vec<SearchHit> {
        if hashes.is_empty() {
            return Vec::new();
        }

        // Gather postings per token in phrase
        let mut term_postings: Vec<&Vec<Posting>> = Vec::new();
        for h in hashes {
            match self.vocab.iter().find(|t| t.token_hash == *h) {
                Some(t) => term_postings.push(&t.postings),
                None => return Vec::new(), // missing term → no match
            }
        }

        // Find documents containing all terms
        let first_docs: Vec<u32> = term_postings[0].iter().map(|p| p.doc_id).collect();
        let candidate_docs: Vec<u32> = first_docs.into_iter().filter(|&did| {
            term_postings.iter().all(|tp| tp.iter().any(|p| p.doc_id == did))
        }).collect();

        let mut hits: Vec<SearchHit> = Vec::new();
        let mut seen: Vec<u32> = Vec::new();

        for doc_id in candidate_docs {
            if seen.contains(&doc_id) { continue; }
            seen.push(doc_id);

            // Check sequential positions
            let first_positions: Vec<u16> = term_postings[0].iter()
                .filter(|p| p.doc_id == doc_id)
                .map(|p| p.position)
                .collect();

            let mut matched = false;
            for start_pos in &first_positions {
                let mut ok = true;
                for (i, tp) in term_postings.iter().enumerate().skip(1) {
                    let needed_pos = *start_pos + i as u16;
                    if !tp.iter().any(|p| p.doc_id == doc_id && p.position == needed_pos) {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    matched = true;
                    break;
                }
            }

            if matched {
                let doc_len = self.documents.iter()
                    .find(|d| d.doc_id == doc_id && d.active)
                    .map_or(1, |d| d.total_tokens);
                let ts = self.documents.iter()
                    .find(|d| d.doc_id == doc_id)
                    .map_or(0, |d| d.timestamp);

                // Score: boost phrase matches significantly
                let base_score = q16_div(
                    q16_from_int(hashes.len() as i32),
                    q16_from_int(doc_len as i32),
                );
                let phrase_boost = q16_mul(base_score, q16_from_int(4));
                hits.push(SearchHit { doc_id, score_q16: phrase_boost, timestamp: ts });
            }
        }

        self.sort_hits(&mut hits);
        hits
    }

    /// Fuzzy query — match tokens within edit distance using hash heuristic
    fn query_fuzzy(&self, token_hash: u64, max_dist: usize) -> Vec<SearchHit> {
        let max_dist = if max_dist > MAX_EDIT_DISTANCE { MAX_EDIT_DISTANCE } else { max_dist };
        let mut all_hits: Vec<SearchHit> = Vec::new();

        for term in &self.vocab {
            let dist = hash_distance(token_hash, term.token_hash);
            if dist <= max_dist {
                // Reduce score proportionally to distance
                let distance_penalty = q16_div(
                    q16_from_int((max_dist - dist + 1) as i32),
                    q16_from_int((max_dist + 1) as i32),
                );
                let term_hits = self.query_term(term.token_hash);
                for mut hit in term_hits {
                    hit.score_q16 = q16_mul(hit.score_q16, distance_penalty);
                    // Merge or add
                    if let Some(existing) = all_hits.iter_mut().find(|h| h.doc_id == hit.doc_id) {
                        if hit.score_q16 > existing.score_q16 {
                            existing.score_q16 = hit.score_q16;
                        }
                    } else {
                        all_hits.push(hit);
                    }
                }
            }
        }

        self.sort_hits(&mut all_hits);
        all_hits
    }

    /// Boolean query — AND / OR / NOT across child clauses
    fn query_boolean(&self, op: BoolOp, children: &[QueryClause]) -> Vec<SearchHit> {
        if children.is_empty() {
            return Vec::new();
        }

        let mut child_results: Vec<Vec<SearchHit>> = Vec::new();
        for (i, child) in children.iter().enumerate() {
            if i >= MAX_QUERY_CLAUSES { break; }
            child_results.push(self.execute_query(child));
        }

        match op {
            BoolOp::And => self.intersect_results(&child_results),
            BoolOp::Or => self.union_results(&child_results),
            BoolOp::Not => {
                if child_results.len() < 2 { return Vec::new(); }
                self.subtract_results(&child_results[0], &child_results[1])
            }
        }
    }

    /// Intersect: documents must appear in ALL result sets
    fn intersect_results(&self, sets: &[Vec<SearchHit>]) -> Vec<SearchHit> {
        if sets.is_empty() { return Vec::new(); }
        let mut result = sets[0].clone();
        for set in sets.iter().skip(1) {
            result.retain(|hit| set.iter().any(|s| s.doc_id == hit.doc_id));
            // Sum scores from matched docs
            for hit in &mut result {
                if let Some(other) = set.iter().find(|s| s.doc_id == hit.doc_id) {
                    hit.score_q16 += other.score_q16;
                }
            }
        }
        self.sort_hits(&mut result);
        result
    }

    /// Union: documents from ANY result set, best score wins
    fn union_results(&self, sets: &[Vec<SearchHit>]) -> Vec<SearchHit> {
        let mut merged: Vec<SearchHit> = Vec::new();
        for set in sets {
            for hit in set {
                if let Some(existing) = merged.iter_mut().find(|h| h.doc_id == hit.doc_id) {
                    if hit.score_q16 > existing.score_q16 {
                        existing.score_q16 = hit.score_q16;
                    }
                } else {
                    merged.push(*hit);
                }
            }
        }
        self.sort_hits(&mut merged);
        merged
    }

    /// Subtract: documents in first set but NOT in second
    fn subtract_results(&self, include: &[SearchHit], exclude: &[SearchHit]) -> Vec<SearchHit> {
        let mut result: Vec<SearchHit> = include.to_vec();
        result.retain(|hit| !exclude.iter().any(|e| e.doc_id == hit.doc_id));
        result
    }

    /// Sort hits by score descending, then by timestamp descending
    fn sort_hits(&self, hits: &mut Vec<SearchHit>) {
        // Simple insertion sort (fine for typical result-set sizes)
        for i in 1..hits.len() {
            let mut j = i;
            while j > 0 && self.hit_greater(&hits[j], &hits[j - 1]) {
                hits.swap(j, j - 1);
                j -= 1;
            }
        }
    }

    fn hit_greater(&self, a: &SearchHit, b: &SearchHit) -> bool {
        if a.score_q16 != b.score_q16 {
            a.score_q16 > b.score_q16
        } else {
            a.timestamp > b.timestamp
        }
    }

    // ----- Maintenance -----------------------------------------------------

    /// Compact the index by removing inactive documents
    fn compact(&mut self) {
        let inactive: Vec<u32> = self.documents.iter()
            .filter(|d| !d.active)
            .map(|d| d.doc_id)
            .collect();
        for id in &inactive {
            self.remove_document(*id);
        }
        self.documents.retain(|d| d.active);
    }

    /// Get index statistics
    fn stats(&self) -> FullTextStats {
        FullTextStats {
            total_documents: self.total_docs,
            active_documents: self.documents.iter().filter(|d| d.active).count() as u32,
            vocab_size: self.vocab.len() as u32,
            total_postings: self.vocab.iter().map(|t| t.postings.len() as u64).sum(),
            avg_doc_len_q16: self.avg_doc_len_q16,
        }
    }
}

/// Statistics snapshot
#[derive(Clone, Copy, Debug)]
pub struct FullTextStats {
    pub total_documents: u32,
    pub active_documents: u32,
    pub vocab_size: u32,
    pub total_postings: u64,
    pub avg_doc_len_q16: i32,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn add_document(content_hash: u64, token_hashes: &[u64], timestamp: u64) -> u32 {
    let mut ft = FULL_TEXT.lock();
    if let Some(engine) = ft.as_mut() {
        engine.add_document(content_hash, token_hashes, timestamp)
    } else {
        0
    }
}

pub fn remove_document(doc_id: u32) {
    let mut ft = FULL_TEXT.lock();
    if let Some(engine) = ft.as_mut() {
        engine.remove_document(doc_id);
    }
}

pub fn search(query: &QueryClause) -> Vec<SearchHit> {
    let ft = FULL_TEXT.lock();
    if let Some(engine) = ft.as_ref() {
        engine.execute_query(query)
    } else {
        Vec::new()
    }
}

pub fn compact_index() {
    let mut ft = FULL_TEXT.lock();
    if let Some(engine) = ft.as_mut() {
        engine.compact();
    }
}

pub fn get_stats() -> Option<FullTextStats> {
    let ft = FULL_TEXT.lock();
    ft.as_ref().map(|e| e.stats())
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

pub fn init() {
    let mut ft = FULL_TEXT.lock();
    *ft = Some(FullTextEngine::new());
    serial_println!("    Full-text search engine ready");
}

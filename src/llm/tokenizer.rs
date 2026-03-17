use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
/// BPE Tokenizer — built from scratch
///
/// Byte-pair encoding with:
///   - Vocabulary learning from corpus (iterative pair counting with real merges)
///   - Pre-tokenization: split on whitespace + punctuation boundaries
///   - Encode text -> token IDs with merge priority lookup
///   - Decode token IDs -> text (with UTF-8 byte fallback)
///   - Special tokens (BOS, EOS, PAD, UNK, SYSTEM, USER, ASSISTANT, TOOL, THINK)
///   - Merge table for fast encoding
///   - Token frequency tracking
///   - Vocabulary serialization/deserialization to/from bytes
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

// =============================================================================
// Data structures
// =============================================================================

/// A merge rule: (left_token, right_token) -> merged_token
#[derive(Clone, Copy)]
struct MergeRule {
    left: u32,
    right: u32,
    merged: u32,
    priority: u32, // Lower = higher priority (earlier merge)
}

/// Vocabulary entry
#[derive(Clone)]
struct VocabEntry {
    id: u32,
    /// Raw bytes that this token decodes to (variable length)
    bytes: Vec<u8>,
    /// Count of how many times this token appeared during encoding
    frequency: u64,
}

/// Pair count entry used during training
#[derive(Clone, Copy)]
struct PairCount {
    left: u32,
    right: u32,
    count: u64,
}

/// Pre-tokenization boundary type
#[derive(Clone, Copy, PartialEq)]
enum CharType {
    Whitespace,
    Punctuation,
    Letter,
    Digit,
    Other,
}

/// Serialized vocabulary format header
#[derive(Clone, Copy)]
struct VocabFileHeader {
    magic: u32, // 0x42504556 = "BPEV"
    version: u16,
    vocab_size: u32,
    n_merges: u32,
    n_special: u16,
}

const VOCAB_MAGIC: u32 = 0x42504556; // "BPEV"
const VOCAB_VERSION: u16 = 1;
const NUM_SPECIAL_TOKENS: u32 = 9;
const BYTE_TOKEN_OFFSET: u32 = 9; // First 9 IDs are special tokens
const BYTE_TOKENS: u32 = 256; // One token per byte value
const BASE_VOCAB_SIZE: u32 = NUM_SPECIAL_TOKENS + BYTE_TOKENS; // 265

pub struct BpeTokenizer {
    vocab: Vec<VocabEntry>,
    merges: Vec<MergeRule>,
    vocab_size: u32,
    /// Merge lookup table: for fast encoding, maps (left_id, right_id) -> (merged_id, priority)
    /// Stored as a flat sorted vec for binary search
    merge_lookup: Vec<(u64, u32, u32)>, // (key, merged_id, priority) where key = (left << 32) | right
    // Special token IDs
    pub bos_id: u32,       // Beginning of sequence
    pub eos_id: u32,       // End of sequence
    pub pad_id: u32,       // Padding
    pub unk_id: u32,       // Unknown
    pub system_id: u32,    // System prompt marker
    pub user_id: u32,      // User turn marker
    pub assistant_id: u32, // Assistant turn marker
    pub tool_id: u32,      // Tool call marker
    pub think_id: u32,     // Thinking/reasoning marker
    total_encoded: u64,
    total_tokens_produced: u64,
}

static TOKENIZER: Mutex<Option<BpeTokenizer>> = Mutex::new(None);

// =============================================================================
// Pre-tokenization helpers
// =============================================================================

fn classify_byte(b: u8) -> CharType {
    match b {
        b' ' | b'\t' | b'\n' | b'\r' => CharType::Whitespace,
        b'!' | b'"' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b','
        | b'-' | b'.' | b'/' | b':' | b';' | b'<' | b'=' | b'>' | b'?' | b'@' | b'[' | b'\\'
        | b']' | b'^' | b'_' | b'`' | b'{' | b'|' | b'}' | b'~' => CharType::Punctuation,
        b'0'..=b'9' => CharType::Digit,
        b'A'..=b'Z' | b'a'..=b'z' => CharType::Letter,
        _ => CharType::Other, // High bytes (UTF-8 continuations)
    }
}

/// Pre-tokenize: split input bytes into segments at word/punctuation boundaries.
/// Each segment is processed independently for BPE merges.
/// Rules:
///   - Whitespace is its own segment (preserves leading space as " word")
///   - Punctuation is its own segment
///   - Runs of letters/digits stay together
///   - UTF-8 multi-byte sequences stay together
fn pre_tokenize(text: &[u8]) -> Vec<(usize, usize)> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;

    while i < text.len() {
        let ct = classify_byte(text[i]);

        match ct {
            CharType::Whitespace => {
                // Emit previous segment if any
                if i > start {
                    segments.push((start, i));
                }
                // Include whitespace with the next word segment (leading space)
                let ws_start = i;
                while i < text.len() && classify_byte(text[i]) == CharType::Whitespace {
                    i += 1;
                }
                // If there are letters/digits following, attach whitespace to them
                if i < text.len() {
                    let next_ct = classify_byte(text[i]);
                    if next_ct == CharType::Letter
                        || next_ct == CharType::Digit
                        || next_ct == CharType::Other
                    {
                        let _word_start = i;
                        while i < text.len() {
                            let c = classify_byte(text[i]);
                            if c == CharType::Letter || c == CharType::Digit || c == CharType::Other
                            {
                                i += 1;
                            } else {
                                break;
                            }
                        }
                        segments.push((ws_start, i));
                    } else {
                        // Whitespace alone
                        segments.push((ws_start, i));
                    }
                } else {
                    segments.push((ws_start, i));
                }
                start = i;
            }
            CharType::Punctuation => {
                if i > start {
                    segments.push((start, i));
                }
                // Each punctuation character is its own segment
                segments.push((i, i + 1));
                i += 1;
                start = i;
            }
            CharType::Letter | CharType::Digit | CharType::Other => {
                // Consume a run of same general type (letters+digits+other)
                while i < text.len() {
                    let c = classify_byte(text[i]);
                    if c == CharType::Letter || c == CharType::Digit || c == CharType::Other {
                        i += 1;
                    } else {
                        break;
                    }
                }
                segments.push((start, i));
                start = i;
            }
        }
    }
    if start < text.len() {
        segments.push((start, text.len()));
    }
    segments
}

// =============================================================================
// BPE Tokenizer implementation
// =============================================================================

impl BpeTokenizer {
    pub fn new(_target_vocab_size: u32) -> Self {
        let mut tok = BpeTokenizer {
            vocab: Vec::new(),
            merges: Vec::new(),
            vocab_size: 0,
            merge_lookup: Vec::new(),
            bos_id: 0,
            eos_id: 1,
            pad_id: 2,
            unk_id: 3,
            system_id: 4,
            user_id: 5,
            assistant_id: 6,
            tool_id: 7,
            think_id: 8,
            total_encoded: 0,
            total_tokens_produced: 0,
        };

        // Initialize with special tokens (IDs 0-8)
        let specials: [&[u8]; 9] = [
            b"<BOS>", b"<EOS>", b"<PAD>", b"<UNK>", b"<SYS>", b"<USR>", b"<AST>", b"<TOL>",
            b"<THK>",
        ];
        for (i, &name) in specials.iter().enumerate() {
            tok.vocab.push(VocabEntry {
                id: i as u32,
                bytes: name.to_vec(),
                frequency: 0,
            });
            tok.vocab_size += 1;
        }

        // Byte-level tokens (IDs 9-264): one token per possible byte value
        for b in 0..=255u8 {
            tok.vocab.push(VocabEntry {
                id: tok.vocab_size,
                bytes: vec![b],
                frequency: 0,
            });
            tok.vocab_size += 1;
        }

        tok
    }

    /// Learn BPE merges from a corpus (byte sequences).
    ///
    /// This is the real BPE training loop:
    /// 1. Start with byte-level tokenization of the corpus
    /// 2. Count all adjacent token pairs
    /// 3. Find the most frequent pair
    /// 4. Merge it (create a new token, replace all occurrences)
    /// 5. Repeat until target_vocab_size or no more merges
    pub fn train(&mut self, corpus: &[u8], target_vocab: u32) {
        if corpus.is_empty() {
            return;
        }

        let merges_to_learn = (target_vocab as usize).saturating_sub(self.vocab_size as usize);
        if merges_to_learn == 0 {
            return;
        }

        // Pre-tokenize the corpus into segments
        let segments = pre_tokenize(corpus);

        // Initialize: convert each segment into byte-level token IDs
        let mut seg_tokens: Vec<Vec<u32>> = Vec::with_capacity(segments.len());
        for &(start, end) in &segments {
            let tokens: Vec<u32> = corpus[start..end]
                .iter()
                .map(|&b| b as u32 + BYTE_TOKEN_OFFSET)
                .collect();
            seg_tokens.push(tokens);
        }

        // Iterative merge loop
        for merge_iter in 0..merges_to_learn {
            // Count all adjacent pairs across all segments
            let mut pair_counts: Vec<PairCount> = Vec::new();

            for tokens in &seg_tokens {
                if tokens.len() < 2 {
                    continue;
                }
                for w in tokens.windows(2) {
                    let left = w[0];
                    let right = w[1];
                    // Find or insert
                    let mut found = false;
                    for pc in pair_counts.iter_mut() {
                        if pc.left == left && pc.right == right {
                            pc.count += 1;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        pair_counts.push(PairCount {
                            left,
                            right,
                            count: 1,
                        });
                    }
                }
            }

            if pair_counts.is_empty() {
                break;
            }

            // Find the most frequent pair
            let mut best_idx = 0usize;
            let mut best_count = 0u64;
            for (i, pc) in pair_counts.iter().enumerate() {
                if pc.count > best_count {
                    best_count = pc.count;
                    best_idx = i;
                }
            }

            if best_count < 2 {
                break;
            } // No pair occurs more than once

            let best_left = pair_counts[best_idx].left;
            let best_right = pair_counts[best_idx].right;
            let new_token_id = self.vocab_size;

            // Create the merged token's bytes by concatenating left + right
            let left_bytes = self.get_token_bytes(best_left);
            let right_bytes = self.get_token_bytes(best_right);
            let mut merged_bytes = left_bytes;
            merged_bytes.extend_from_slice(&right_bytes);

            self.vocab.push(VocabEntry {
                id: new_token_id,
                bytes: merged_bytes,
                frequency: best_count,
            });

            self.merges.push(MergeRule {
                left: best_left,
                right: best_right,
                merged: new_token_id,
                priority: merge_iter as u32,
            });

            self.vocab_size = self.vocab_size.saturating_add(1);

            // Apply this merge to all segments
            for tokens in seg_tokens.iter_mut() {
                apply_merge(tokens, best_left, best_right, new_token_id);
            }

            // Early termination if we've reached target
            if self.vocab_size >= target_vocab {
                break;
            }
        }

        // Rebuild merge lookup table for fast encoding
        self.rebuild_merge_lookup();
    }

    /// Get the raw bytes for a token ID
    fn get_token_bytes(&self, id: u32) -> Vec<u8> {
        if let Some(entry) = self.vocab.iter().find(|v| v.id == id) {
            entry.bytes.clone()
        } else {
            Vec::new()
        }
    }

    /// Rebuild the fast merge lookup table from the merge list.
    /// Encodes (left, right) as a u64 key for binary search.
    fn rebuild_merge_lookup(&mut self) {
        self.merge_lookup.clear();
        self.merge_lookup.reserve(self.merges.len());
        for m in &self.merges {
            let key = ((m.left as u64) << 32) | (m.right as u64);
            self.merge_lookup.push((key, m.merged, m.priority));
        }
        // Sort by key for binary search
        self.merge_lookup.sort_by_key(|e| e.0);
    }

    /// Look up a merge rule by (left, right) token pair.
    /// Returns (merged_id, priority) if found.
    fn lookup_merge(&self, left: u32, right: u32) -> Option<(u32, u32)> {
        let key = ((left as u64) << 32) | (right as u64);
        // Binary search
        let mut lo = 0usize;
        let mut hi = self.merge_lookup.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.merge_lookup[mid].0 < key {
                lo = mid + 1;
            } else if self.merge_lookup[mid].0 > key {
                hi = mid;
            } else {
                return Some((self.merge_lookup[mid].1, self.merge_lookup[mid].2));
            }
        }
        None
    }

    /// Encode text to token IDs using pre-tokenization and BPE merges.
    pub fn encode(&mut self, text: &[u8]) -> Vec<u32> {
        self.total_encoded = self.total_encoded.saturating_add(1);

        if text.is_empty() {
            return Vec::new();
        }

        let segments = pre_tokenize(text);
        let mut all_tokens: Vec<u32> = Vec::new();

        for &(start, end) in &segments {
            // Start with byte-level tokens for this segment
            let mut tokens: Vec<u32> = text[start..end]
                .iter()
                .map(|&b| b as u32 + BYTE_TOKEN_OFFSET)
                .collect();

            // Apply merges using priority-based approach
            // We iterate merges from highest priority (lowest number) to lowest
            if !self.merge_lookup.is_empty() {
                // Multi-pass: keep applying merges until no more apply
                let mut changed = true;
                while changed {
                    changed = false;
                    let mut best_pos = 0usize;
                    let mut best_priority = u32::MAX;
                    let mut best_merged = 0u32;
                    let mut found = false;

                    // Scan for the highest-priority applicable merge
                    for i in 0..tokens.len().saturating_sub(1) {
                        if let Some((merged, priority)) =
                            self.lookup_merge(tokens[i], tokens[i + 1])
                        {
                            if priority < best_priority {
                                best_priority = priority;
                                best_pos = i;
                                best_merged = merged;
                                found = true;
                            }
                        }
                    }

                    if found {
                        tokens[best_pos] = best_merged;
                        tokens.remove(best_pos + 1);
                        changed = true;
                    }
                }
            }

            // Track frequency of produced tokens
            for &tid in &tokens {
                if let Some(entry) = self.vocab.iter_mut().find(|v| v.id == tid) {
                    entry.frequency += 1;
                }
            }

            self.total_tokens_produced += tokens.len() as u64;
            all_tokens.extend_from_slice(&tokens);
        }

        all_tokens
    }

    /// Decode token IDs back to bytes
    pub fn decode(&self, tokens: &[u32]) -> Vec<u8> {
        let mut output = Vec::new();
        for &tid in tokens {
            if let Some(entry) = self.vocab.iter().find(|v| v.id == tid) {
                output.extend_from_slice(&entry.bytes);
            }
            // Unknown tokens are silently dropped (could emit UNK bytes instead)
        }
        output
    }

    /// Get current vocabulary size
    pub fn vocab_size(&self) -> u32 {
        self.vocab_size
    }

    /// Get the number of learned merge rules
    pub fn merge_count(&self) -> u32 {
        self.merges.len() as u32
    }

    /// Get total tokens encoded
    pub fn total_encoded(&self) -> u64 {
        self.total_encoded
    }

    /// Get total tokens produced (after merges)
    pub fn total_tokens_produced(&self) -> u64 {
        self.total_tokens_produced
    }

    /// Get frequency of a specific token
    pub fn token_frequency(&self, id: u32) -> u64 {
        self.vocab
            .iter()
            .find(|v| v.id == id)
            .map_or(0, |v| v.frequency)
    }

    /// Get the N most frequent tokens (excluding special and byte tokens)
    pub fn top_tokens(&self, n: usize) -> Vec<(u32, u64)> {
        let mut entries: Vec<(u32, u64)> = self
            .vocab
            .iter()
            .filter(|v| v.id >= BASE_VOCAB_SIZE && v.frequency > 0)
            .map(|v| (v.id, v.frequency))
            .collect();
        // Sort by frequency descending
        for i in 1..entries.len() {
            let key = entries[i];
            let mut j = i;
            while j > 0 && entries[j - 1].1 < key.1 {
                entries[j] = entries[j - 1];
                j -= 1;
            }
            entries[j] = key;
        }
        entries.truncate(n);
        entries
    }

    /// Serialize the vocabulary and merge table to bytes.
    /// Format:
    ///   [VocabFileHeader]
    ///   For each vocab entry: [id: u32] [byte_len: u16] [bytes...]
    ///   For each merge: [left: u32] [right: u32] [merged: u32] [priority: u32]
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Header
        buf.extend_from_slice(&VOCAB_MAGIC.to_le_bytes());
        buf.extend_from_slice(&VOCAB_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.vocab_size.to_le_bytes());
        buf.extend_from_slice(&(self.merges.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(NUM_SPECIAL_TOKENS as u16).to_le_bytes());

        // Vocabulary entries
        for entry in &self.vocab {
            buf.extend_from_slice(&entry.id.to_le_bytes());
            let blen = entry.bytes.len() as u16;
            buf.extend_from_slice(&blen.to_le_bytes());
            buf.extend_from_slice(&entry.bytes);
        }

        // Merge rules
        for merge in &self.merges {
            buf.extend_from_slice(&merge.left.to_le_bytes());
            buf.extend_from_slice(&merge.right.to_le_bytes());
            buf.extend_from_slice(&merge.merged.to_le_bytes());
            buf.extend_from_slice(&merge.priority.to_le_bytes());
        }

        buf
    }

    /// Deserialize vocabulary and merges from bytes.
    /// Returns a new BpeTokenizer with the loaded data.
    pub fn deserialize(data: &[u8]) -> Option<Self> {
        if data.len() < 16 {
            return None;
        }
        let mut offset = 0usize;

        // Read header
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic != VOCAB_MAGIC {
            return None;
        }
        offset += 4;

        let version = u16::from_le_bytes([data[offset], data[offset + 1]]);
        if version > VOCAB_VERSION {
            return None;
        }
        offset += 2;

        let vocab_size = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;

        let n_merges = u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);
        offset += 4;

        let _n_special = u16::from_le_bytes([data[offset], data[offset + 1]]);
        offset += 2;

        // Read vocab entries
        let mut vocab = Vec::with_capacity(vocab_size as usize);
        for _ in 0..vocab_size {
            if offset + 6 > data.len() {
                return None;
            }
            let id = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            let blen = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
            offset += 2;
            if offset + blen > data.len() {
                return None;
            }
            let bytes = data[offset..offset + blen].to_vec();
            offset += blen;
            vocab.push(VocabEntry {
                id,
                bytes,
                frequency: 0,
            });
        }

        // Read merge rules
        let mut merges = Vec::with_capacity(n_merges as usize);
        for _ in 0..n_merges {
            if offset + 16 > data.len() {
                return None;
            }
            let left = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            let right = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            let merged = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            let priority = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            offset += 4;
            merges.push(MergeRule {
                left,
                right,
                merged,
                priority,
            });
        }

        let mut tok = BpeTokenizer {
            vocab,
            merges,
            vocab_size,
            merge_lookup: Vec::new(),
            bos_id: 0,
            eos_id: 1,
            pad_id: 2,
            unk_id: 3,
            system_id: 4,
            user_id: 5,
            assistant_id: 6,
            tool_id: 7,
            think_id: 8,
            total_encoded: 0,
            total_tokens_produced: 0,
        };
        tok.rebuild_merge_lookup();
        Some(tok)
    }

    /// Reset frequency counters for all tokens
    pub fn reset_frequencies(&mut self) {
        for entry in &mut self.vocab {
            entry.frequency = 0;
        }
    }

    /// Compression ratio: input bytes / output tokens (as Q16 fixed-point)
    pub fn compression_ratio(&self, input_len: u64, token_count: u64) -> i32 {
        if token_count == 0 {
            return 0;
        }
        ((input_len as i64 * 65536) / token_count as i64) as i32
    }
}

/// Apply a single merge rule to a token sequence in-place.
fn apply_merge(tokens: &mut Vec<u32>, left: u32, right: u32, merged: u32) {
    let mut i = 0;
    while i + 1 < tokens.len() {
        if tokens[i] == left && tokens[i + 1] == right {
            tokens[i] = merged;
            tokens.remove(i + 1);
            // Don't advance: check if new token can merge with next
        } else {
            i += 1;
        }
    }
}

// =============================================================================
// Public API (global tokenizer)
// =============================================================================

pub fn init() {
    let mut t = TOKENIZER.lock();
    *t = Some(BpeTokenizer::new(32_000));
    serial_println!(
        "    BPE tokenizer: {} base + special tokens, pre-tokenize, merge-ready",
        BASE_VOCAB_SIZE
    );
}

/// Encode UTF-8 text into token IDs with the global tokenizer.
pub fn encode_text(text: &str) -> Option<Vec<u32>> {
    TOKENIZER
        .lock()
        .as_mut()
        .map(|tok| tok.encode(text.as_bytes()))
}

/// Decode token IDs into a mostly printable UTF-8-ish string.
pub fn decode_tokens(tokens: &[u32]) -> Option<String> {
    TOKENIZER.lock().as_ref().map(|tok| {
        let bytes = tok.decode(tokens);
        // Attempt UTF-8 decode with byte fallback
        let mut text = String::new();
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x80 {
                // ASCII
                if b == b'\n' || b == b'\r' || b == b'\t' || (32..=126).contains(&b) {
                    text.push(b as char);
                } else {
                    text.push('\u{FFFD}'); // Replacement character for non-printable
                }
                i += 1;
            } else if b & 0xE0 == 0xC0 && i + 1 < bytes.len() {
                // 2-byte UTF-8
                let c = ((b as u32 & 0x1F) << 6) | (bytes[i + 1] as u32 & 0x3F);
                if let Some(ch) = char::from_u32(c) {
                    text.push(ch);
                } else {
                    text.push('\u{FFFD}');
                }
                i += 2;
            } else if b & 0xF0 == 0xE0 && i + 2 < bytes.len() {
                // 3-byte UTF-8
                let c = ((b as u32 & 0x0F) << 12)
                    | ((bytes[i + 1] as u32 & 0x3F) << 6)
                    | (bytes[i + 2] as u32 & 0x3F);
                if let Some(ch) = char::from_u32(c) {
                    text.push(ch);
                } else {
                    text.push('\u{FFFD}');
                }
                i += 3;
            } else if b & 0xF8 == 0xF0 && i + 3 < bytes.len() {
                // 4-byte UTF-8
                let c = ((b as u32 & 0x07) << 18)
                    | ((bytes[i + 1] as u32 & 0x3F) << 12)
                    | ((bytes[i + 2] as u32 & 0x3F) << 6)
                    | (bytes[i + 3] as u32 & 0x3F);
                if let Some(ch) = char::from_u32(c) {
                    text.push(ch);
                } else {
                    text.push('\u{FFFD}');
                }
                i += 4;
            } else {
                // Invalid UTF-8 byte: use replacement
                text.push('\u{FFFD}');
                i += 1;
            }
        }
        text
    })
}

pub fn eos_token_id() -> Option<u32> {
    TOKENIZER.lock().as_ref().map(|tok| tok.eos_id)
}

pub fn bos_token_id() -> Option<u32> {
    TOKENIZER.lock().as_ref().map(|tok| tok.bos_id)
}

/// Get current vocabulary size
pub fn vocab_size() -> u32 {
    TOKENIZER.lock().as_ref().map_or(0, |tok| tok.vocab_size())
}

/// Get the number of learned merges
pub fn merge_count() -> u32 {
    TOKENIZER.lock().as_ref().map_or(0, |tok| tok.merge_count())
}

/// Train the global tokenizer on a corpus
pub fn train(corpus: &[u8], target_vocab: u32) {
    if let Some(tok) = TOKENIZER.lock().as_mut() {
        tok.train(corpus, target_vocab);
    }
}

/// Serialize the global tokenizer to bytes
pub fn serialize() -> Option<Vec<u8>> {
    TOKENIZER.lock().as_ref().map(|tok| tok.serialize())
}

/// Load a tokenizer from serialized bytes into the global slot
pub fn load(data: &[u8]) -> bool {
    if let Some(tok) = BpeTokenizer::deserialize(data) {
        *TOKENIZER.lock() = Some(tok);
        true
    } else {
        false
    }
}

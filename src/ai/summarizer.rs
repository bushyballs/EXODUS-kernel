use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
/// Text summarization engine
///
/// Part of the Hoags AI subsystem. Extractive text summarizer
/// that splits text into sentences, scores sentences by word frequency,
/// and selects top-K sentences maintaining original order.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Stop words to exclude from frequency scoring
const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "above", "below", "between", "and", "but", "or", "nor", "not", "no", "so",
    "if", "then", "than", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
    "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "what",
    "which", "who", "whom", "where", "when", "how", "why", "all", "each", "every", "both", "few",
    "more", "most", "other", "some", "such", "only", "own", "same", "also", "just", "about", "up",
    "out", "any", "here", "there", "very", "really", "just", "like", "much", "many", "even",
    "still", "already", "too",
];

/// A scored sentence for ranking
struct ScoredSentence {
    index: usize,
    text: String,
    score: f32,
    word_count: usize,
    position_score: f32,
}

pub struct Summarizer {
    pub max_length: usize,
    /// Minimum sentence length in words to be considered for summary
    min_sentence_words: usize,
    /// Weight for sentence position (beginning and end of document score higher)
    position_weight: f32,
    /// Weight for word frequency scoring
    frequency_weight: f32,
    /// Weight for sentence length (prefer medium-length sentences)
    length_weight: f32,
    /// Similarity threshold for redundancy elimination
    redundancy_threshold: f32,
    /// Total summarizations performed
    total_summarizations: u64,
}

impl Summarizer {
    pub fn new() -> Self {
        Summarizer {
            max_length: 3,
            min_sentence_words: 4,
            position_weight: 0.15,
            frequency_weight: 0.6,
            length_weight: 0.1,
            redundancy_threshold: 0.6,
            total_summarizations: 0,
        }
    }

    /// Create with custom max sentence count
    pub fn with_max_sentences(max: usize) -> Self {
        let mut s = Self::new();
        s.max_length = max;
        s
    }

    /// Set weights for different scoring components
    pub fn set_weights(&mut self, frequency: f32, position: f32, length: f32) {
        self.frequency_weight = frequency;
        self.position_weight = position;
        self.length_weight = length;
    }

    /// Summarize the input text, selecting top sentences up to max_length
    pub fn summarize(&self, text: &str) -> String {
        let sentences = self.extract_key_sentences(text, self.max_length);
        if sentences.is_empty() {
            // If text is short enough, return as-is
            if text.len() < 500 {
                return String::from(text);
            }
            return String::new();
        }
        sentences.join(" ")
    }

    /// Summarize to a target word count
    pub fn summarize_to_words(&self, text: &str, target_words: usize) -> String {
        let sentences = split_sentences(text);
        if sentences.is_empty() {
            return String::new();
        }

        let word_freq = build_word_frequency(text);
        let scored = self.score_sentences(&sentences, &word_freq);

        // Select sentences until we approach the target word count
        let mut ranked = scored;
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        let mut selected_indices: Vec<usize> = Vec::new();
        let mut total_words = 0;

        for sent in &ranked {
            if total_words + sent.word_count > target_words && !selected_indices.is_empty() {
                break;
            }
            // Redundancy check
            if !self.is_redundant(sent, &selected_indices, &ranked) {
                selected_indices.push(sent.index);
                total_words += sent.word_count;
            }
        }

        // Sort by original order
        selected_indices.sort();

        let summary: Vec<String> = selected_indices
            .iter()
            .filter_map(|&idx| sentences.get(idx).map(|s| s.clone()))
            .collect();

        summary.join(" ")
    }

    /// Extract the top-N most important sentences, maintaining original order
    pub fn extract_key_sentences(&self, text: &str, n: usize) -> Vec<String> {
        let sentences = split_sentences(text);
        if sentences.is_empty() {
            return Vec::new();
        }
        if sentences.len() <= n {
            return sentences;
        }

        let word_freq = build_word_frequency(text);
        let scored = self.score_sentences(&sentences, &word_freq);

        // Sort by score descending
        let mut ranked = scored;
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        // Select top-N with redundancy elimination
        let mut selected_indices: Vec<usize> = Vec::new();
        for sent in &ranked {
            if selected_indices.len() >= n {
                break;
            }
            if !self.is_redundant(sent, &selected_indices, &ranked) {
                selected_indices.push(sent.index);
            }
        }

        // Sort by original order for coherent reading
        selected_indices.sort();

        selected_indices
            .iter()
            .filter_map(|&idx| sentences.get(idx).map(|s| s.clone()))
            .collect()
    }

    /// Score all sentences
    fn score_sentences(
        &self,
        sentences: &[String],
        word_freq: &BTreeMap<String, f32>,
    ) -> Vec<ScoredSentence> {
        let total_sentences = sentences.len() as f32;

        sentences
            .iter()
            .enumerate()
            .filter_map(|(idx, sentence)| {
                let words = extract_words(sentence);
                let word_count = words.len();

                if word_count < self.min_sentence_words {
                    return None;
                }

                // Frequency score: sum of word frequencies for non-stop words
                let freq_score = if words.is_empty() {
                    0.0
                } else {
                    let sum: f32 = words
                        .iter()
                        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()))
                        .map(|w| word_freq.get(w).copied().unwrap_or(0.0))
                        .sum();
                    sum / sqrt_f32(word_count as f32) // Normalize by sqrt of length
                };

                // Position score: first and last sentences are typically more important
                let position_score = if total_sentences <= 1.0 {
                    1.0
                } else {
                    let _pos_frac = idx as f32 / total_sentences;
                    // U-shaped curve: high at beginning and end
                    let beginning_bonus = if idx < 3 {
                        0.3 * (3.0 - idx as f32) / 3.0
                    } else {
                        0.0
                    };
                    let end_bonus = if idx >= sentences.len() - 2 {
                        0.15
                    } else {
                        0.0
                    };
                    // First sentence always gets a bonus
                    let first_bonus = if idx == 0 { 0.2 } else { 0.0 };
                    beginning_bonus + end_bonus + first_bonus
                };

                // Length score: prefer medium-length sentences (not too short, not too long)
                let ideal_length = 20.0f32;
                let length_diff = (word_count as f32 - ideal_length).abs();
                let length_score = 1.0 / (1.0 + length_diff * 0.05);

                // Combined score
                let total_score = self.frequency_weight * freq_score
                    + self.position_weight * position_score
                    + self.length_weight * length_score;

                Some(ScoredSentence {
                    index: idx,
                    text: sentence.clone(),
                    score: total_score,
                    word_count,
                    position_score,
                })
            })
            .collect()
    }

    /// Check if a sentence is too similar to already-selected sentences
    fn is_redundant(
        &self,
        candidate: &ScoredSentence,
        selected_indices: &[usize],
        all_scored: &[ScoredSentence],
    ) -> bool {
        let candidate_words = extract_words(&candidate.text);
        let candidate_set: Vec<&String> = candidate_words
            .iter()
            .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()))
            .collect();

        if candidate_set.is_empty() {
            return false;
        }

        for &sel_idx in selected_indices {
            let selected = match all_scored.iter().find(|s| s.index == sel_idx) {
                Some(s) => s,
                None => continue,
            };

            let selected_words = extract_words(&selected.text);
            let selected_set: Vec<&String> = selected_words
                .iter()
                .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()))
                .collect();

            if selected_set.is_empty() {
                continue;
            }

            // Jaccard similarity
            let overlap = candidate_set
                .iter()
                .filter(|w| selected_set.contains(w))
                .count();
            let union = candidate_set.len() + selected_set.len() - overlap;
            let similarity = if union > 0 {
                overlap as f32 / union as f32
            } else {
                0.0
            };

            if similarity > self.redundancy_threshold {
                return true;
            }
        }

        false
    }

    /// Get a brief summary with metadata
    pub fn summarize_with_meta(&self, text: &str) -> (String, usize, usize, f32) {
        let _sentences = split_sentences(text);
        let total_words = text.split_whitespace().count();
        let summary = self.summarize(text);
        let summary_words = summary.split_whitespace().count();
        let compression = if total_words > 0 {
            1.0 - (summary_words as f32 / total_words as f32)
        } else {
            0.0
        };

        (summary, total_words, summary_words, compression)
    }

    /// Total summarizations performed
    pub fn total_summarizations(&self) -> u64 {
        self.total_summarizations
    }
}

// ---------------------------------------------------------------------------
// Text processing helpers
// ---------------------------------------------------------------------------

/// Split text into sentences
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        // Only process ASCII safely
        if b > 127 {
            current.push('?');
            i += 1;
            continue;
        }

        current.push(b as char);

        if b == b'.' || b == b'!' || b == b'?' {
            // Consume additional sentence-ending punctuation
            while i + 1 < len
                && (bytes[i + 1] == b'.' || bytes[i + 1] == b'!' || bytes[i + 1] == b'?')
            {
                i += 1;
                current.push(bytes[i] as char);
            }

            // Check for abbreviation: single letter before dot
            let is_abbrev = b == b'.' && current.trim().len() <= 2;

            // Check for decimal number: digit before and after dot
            let is_decimal = b == b'.'
                && i > 0
                && i + 1 < len
                && bytes[i - 1].is_ascii_digit()
                && bytes[i + 1].is_ascii_digit();

            if !is_abbrev && !is_decimal {
                let trimmed = String::from(current.trim());
                if !trimmed.is_empty() {
                    sentences.push(trimmed);
                }
                current = String::new();
            }
        } else if b == b'\n' {
            // Double newline can be a sentence break
            if i + 1 < len && bytes[i + 1] == b'\n' {
                let trimmed = String::from(current.trim());
                if !trimmed.is_empty() && count_words(&trimmed) >= 3 {
                    sentences.push(trimmed);
                }
                current = String::new();
                i += 1; // Skip second newline
            }
        }

        i += 1;
    }

    // Remaining text
    let trimmed = String::from(current.trim());
    if !trimmed.is_empty() && count_words(&trimmed) >= 3 {
        sentences.push(trimmed);
    }

    sentences
}

/// Extract lowercase words from text
fn extract_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            if c.is_ascii_uppercase() {
                current.push((c as u8 + 32) as char);
            } else {
                current.push(c);
            }
        } else {
            if !current.is_empty() {
                words.push(core::mem::replace(&mut current, String::new()));
            }
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Build word frequency map (term frequency normalized)
fn build_word_frequency(text: &str) -> BTreeMap<String, f32> {
    let words = extract_words(text);
    let mut freq: BTreeMap<String, u32> = BTreeMap::new();

    for w in &words {
        if w.len() < 2 {
            continue;
        }
        if STOP_WORDS.contains(&w.as_str()) {
            continue;
        }
        *freq.entry(w.clone()).or_insert(0) += 1;
    }

    // Normalize by max frequency
    let max_freq = freq.values().copied().max().unwrap_or(1).max(1);

    freq.into_iter()
        .map(|(word, count)| (word, count as f32 / max_freq as f32))
        .collect()
}

/// Count words in text
fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x / 2.0;
    for _ in 0..32 {
        let next = 0.5 * (guess + x / guess);
        if (next - guess).abs() < 1e-7 {
            break;
        }
        guess = next;
    }
    guess
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static SUMMARIZER: Mutex<Option<Summarizer>> = Mutex::new(None);

pub fn init() {
    *SUMMARIZER.lock() = Some(Summarizer::new());
    crate::serial_println!(
        "    [summarizer] Extractive summarizer ready (freq+position+length scoring)"
    );
}

/// Summarize text using the global summarizer
pub fn summarize(text: &str) -> String {
    SUMMARIZER
        .lock()
        .as_ref()
        .map(|s| s.summarize(text))
        .unwrap_or_else(String::new)
}

/// Extract top-N key sentences
pub fn extract_key_sentences(text: &str, n: usize) -> Vec<String> {
    SUMMARIZER
        .lock()
        .as_ref()
        .map(|s| s.extract_key_sentences(text, n))
        .unwrap_or_else(Vec::new)
}

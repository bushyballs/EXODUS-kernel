use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// PDF and document analysis for Bid Command integration
///
/// Part of the Hoags AI subsystem. Extracts structured data
/// from solicitation documents (PDF, DOCX) for bid workflows.
///
/// Provides: word count, sentence count, readability scoring
/// (Flesch-Kincaid approximation), keyword extraction (TF-based),
/// and extractive summary by selecting top-scoring sentences.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Helper math functions for no_std
// ---------------------------------------------------------------------------

fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    for _ in 0..10 {
        guess = (guess + x / guess) * 0.5;
    }
    guess
}

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Extracted document structure and metadata
pub struct DocumentAnalysis {
    pub title: String,
    pub sections: Vec<String>,
    pub page_count: usize,
    /// Total word count
    pub word_count: usize,
    /// Total sentence count
    pub sentence_count: usize,
    /// Total character count (excluding whitespace)
    pub char_count: usize,
    /// Total syllable count (estimated)
    pub syllable_count: usize,
    /// Flesch-Kincaid reading ease score
    pub readability_score: f32,
    /// Flesch-Kincaid grade level
    pub grade_level: f32,
    /// Extracted keywords with TF scores
    pub keywords: Vec<(String, f32)>,
    /// Extractive summary (top sentences)
    pub summary_sentences: Vec<String>,
    /// Average sentence length in words
    pub avg_sentence_length: f32,
    /// Average word length in characters
    pub avg_word_length: f32,
    /// Paragraph count
    pub paragraph_count: usize,
}

/// Stop words to exclude from keyword extraction
const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "above", "below", "between", "and", "but", "or", "nor", "not", "no", "so",
    "if", "then", "than", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
    "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their", "what",
    "which", "who", "whom", "where", "when", "how", "why", "all", "each", "every", "both", "few",
    "more", "most", "other", "some", "such", "only", "own", "same", "also", "just", "about", "up",
    "out", "any", "here", "there",
];

impl DocumentAnalysis {
    pub fn new() -> Self {
        DocumentAnalysis {
            title: String::new(),
            sections: Vec::new(),
            page_count: 0,
            word_count: 0,
            sentence_count: 0,
            char_count: 0,
            syllable_count: 0,
            readability_score: 0.0,
            grade_level: 0.0,
            keywords: Vec::new(),
            summary_sentences: Vec::new(),
            avg_sentence_length: 0.0,
            avg_word_length: 0.0,
            paragraph_count: 0,
        }
    }

    /// Analyze raw document bytes and extract structured content.
    /// Interprets the bytes as UTF-8 text for analysis.
    pub fn analyze(&mut self, data: &[u8]) -> Result<(), ()> {
        // Attempt to interpret data as UTF-8 text
        let text = match core::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => {
                // Try to recover: filter to valid ASCII
                let ascii: Vec<u8> = data.iter().filter(|&&b| b < 128).copied().collect();
                match core::str::from_utf8(&ascii) {
                    Ok(_) => {
                        // We need to own the data, so process inline
                        return self.analyze_text_bytes(&ascii);
                    }
                    Err(_) => return Err(()),
                }
            }
        };

        self.analyze_text(text);
        Ok(())
    }

    /// Analyze text content directly
    pub fn analyze_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        // Extract title: first non-empty line
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                self.title = String::from(trimmed);
                break;
            }
        }

        // Count paragraphs (separated by blank lines)
        self.paragraph_count = count_paragraphs(text);

        // Estimate page count (~3000 chars per page)
        self.page_count = (text.len() / 3000).max(1);

        // Split into sections (by double newline or heading-like patterns)
        self.sections = extract_sections(text);

        // Word-level analysis
        let words = extract_words(text);
        self.word_count = words.len();

        // Character count (non-whitespace)
        self.char_count = text.chars().filter(|c| !c.is_whitespace()).count();

        // Average word length
        if self.word_count > 0 {
            let total_word_chars: usize = words.iter().map(|w| w.len()).sum();
            self.avg_word_length = total_word_chars as f32 / self.word_count as f32;
        }

        // Syllable count
        self.syllable_count = words.iter().map(|w| count_syllables(w)).sum();

        // Sentence analysis
        let sentences = split_sentences(text);
        self.sentence_count = sentences.len().max(1);

        // Average sentence length
        if self.sentence_count > 0 {
            self.avg_sentence_length = self.word_count as f32 / self.sentence_count as f32;
        }

        // Readability scoring
        self.compute_readability();

        // Keyword extraction (TF-based)
        self.keywords = extract_keywords(&words, 15);

        // Extractive summary: top 3 sentences by word-frequency score
        self.summary_sentences = extractive_summary(text, &sentences, 3);
    }

    /// Compute Flesch-Kincaid readability scores
    fn compute_readability(&mut self) {
        if self.word_count == 0 || self.sentence_count == 0 {
            self.readability_score = 0.0;
            self.grade_level = 0.0;
            return;
        }

        let words_per_sentence = self.word_count as f32 / self.sentence_count as f32;
        let syllables_per_word = self.syllable_count as f32 / self.word_count as f32;

        // Flesch Reading Ease = 206.835 - 1.015 * (words/sentence) - 84.6 * (syllables/word)
        self.readability_score = 206.835 - 1.015 * words_per_sentence - 84.6 * syllables_per_word;

        // Clamp to [0, 100]
        if self.readability_score < 0.0 {
            self.readability_score = 0.0;
        }
        if self.readability_score > 100.0 {
            self.readability_score = 100.0;
        }

        // Flesch-Kincaid Grade Level = 0.39 * (words/sentence) + 11.8 * (syllables/word) - 15.59
        self.grade_level = 0.39 * words_per_sentence + 11.8 * syllables_per_word - 15.59;

        if self.grade_level < 0.0 {
            self.grade_level = 0.0;
        }
    }

    /// Get a readability description string
    pub fn readability_description(&self) -> String {
        if self.readability_score >= 90.0 {
            String::from("Very Easy (5th grade)")
        } else if self.readability_score >= 80.0 {
            String::from("Easy (6th grade)")
        } else if self.readability_score >= 70.0 {
            String::from("Fairly Easy (7th grade)")
        } else if self.readability_score >= 60.0 {
            String::from("Standard (8th-9th grade)")
        } else if self.readability_score >= 50.0 {
            String::from("Fairly Difficult (10th-12th grade)")
        } else if self.readability_score >= 30.0 {
            String::from("Difficult (College)")
        } else {
            String::from("Very Difficult (Graduate)")
        }
    }

    /// Analyze from raw ASCII bytes (fallback path)
    fn analyze_text_bytes(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let mut text = String::new();
        for &b in bytes {
            if b == b'\n' || b == b'\r' || b == b'\t' || (32..=126).contains(&b) {
                text.push(b as char);
            }
        }
        self.analyze_text(&text);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Text processing helpers
// ---------------------------------------------------------------------------

/// Extract words from text, lowercased, alphabetic only
fn extract_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current_word = String::new();

    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' {
            if c.is_ascii_uppercase() {
                current_word.push((c as u8 + 32) as char);
            } else {
                current_word.push(c);
            }
        } else {
            if !current_word.is_empty() {
                words.push(core::mem::replace(&mut current_word, String::new()));
            }
        }
    }
    if !current_word.is_empty() {
        words.push(current_word);
    }
    words
}

/// Split text into sentences
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        current.push(b as char);

        if b == b'.' || b == b'!' || b == b'?' {
            // Consume additional sentence-ending punctuation
            while i + 1 < len
                && (bytes[i + 1] == b'.' || bytes[i + 1] == b'!' || bytes[i + 1] == b'?')
            {
                i += 1;
                current.push(bytes[i] as char);
            }

            let trimmed = String::from(current.trim());
            if !trimmed.is_empty() && count_words_in(&trimmed) >= 2 {
                sentences.push(trimmed);
            }
            current = String::new();
        }
        i += 1;
    }

    // Remaining text as a sentence if substantial
    let trimmed = String::from(current.trim());
    if !trimmed.is_empty() && count_words_in(&trimmed) >= 3 {
        sentences.push(trimmed);
    }

    sentences
}

fn count_words_in(s: &str) -> usize {
    s.split_whitespace().count()
}

/// Count syllables in a word (English approximation)
fn count_syllables(word: &str) -> usize {
    if word.is_empty() {
        return 0;
    }
    if word.len() <= 3 {
        return 1;
    }

    let lower: Vec<u8> = word
        .bytes()
        .map(|b| if b.is_ascii_uppercase() { b + 32 } else { b })
        .collect();

    let mut count = 0usize;
    let mut prev_vowel = false;

    for (i, &b) in lower.iter().enumerate() {
        let is_vowel = matches!(b, b'a' | b'e' | b'i' | b'o' | b'u' | b'y');
        if is_vowel && !prev_vowel {
            count += 1;
        }
        prev_vowel = is_vowel;

        // Silent 'e' at end
        if i == lower.len() - 1 && b == b'e' && count > 1 {
            count -= 1;
        }
    }

    count.max(1)
}

/// Count paragraphs (text blocks separated by blank lines)
fn count_paragraphs(text: &str) -> usize {
    let mut count = 0;
    let mut in_paragraph = false;

    for line in text.lines() {
        if line.trim().is_empty() {
            if in_paragraph {
                in_paragraph = false;
            }
        } else {
            if !in_paragraph {
                count += 1;
                in_paragraph = true;
            }
        }
    }
    count.max(1)
}

/// Extract sections from text (split on double newlines or heading patterns)
fn extract_sections(text: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut current_section = String::new();

    for line in text.lines() {
        let trimmed = line.trim();

        // Detect section breaks: blank lines or heading-like patterns
        let is_heading = trimmed.starts_with('#')
            || (trimmed.len() > 2
                && trimmed.len() < 80
                && trimmed
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == ' ' || c == ':' || c == '-')
                && trimmed
                    .chars()
                    .next()
                    .map(|c| c.is_uppercase())
                    .unwrap_or(false)
                && !trimmed.contains('.'));

        if trimmed.is_empty() && !current_section.trim().is_empty() {
            sections.push(String::from(current_section.trim()));
            current_section = String::new();
        } else if is_heading && !current_section.trim().is_empty() {
            sections.push(String::from(current_section.trim()));
            current_section = String::from(line);
            current_section.push('\n');
        } else {
            current_section.push_str(line);
            current_section.push('\n');
        }
    }

    if !current_section.trim().is_empty() {
        sections.push(String::from(current_section.trim()));
    }

    sections
}

/// Extract top-N keywords using term frequency
fn extract_keywords(words: &[String], top_n: usize) -> Vec<(String, f32)> {
    if words.is_empty() {
        return Vec::new();
    }

    let mut freq: BTreeMap<String, u32> = BTreeMap::new();
    for w in words {
        if w.len() < 3 {
            continue;
        }
        if STOP_WORDS.contains(&w.as_str()) {
            continue;
        }
        *freq.entry(w.clone()).or_insert(0) += 1;
    }

    let total = words.len() as f32;
    let mut scored: Vec<(String, f32)> = freq
        .into_iter()
        .map(|(word, count)| {
            // TF score, with a small boost for longer words (more specific)
            let tf = count as f32 / total;
            let length_bonus = (word.len() as f32 / 10.0).min(0.5);
            (word, tf + tf * length_bonus)
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
    scored.truncate(top_n);
    scored
}

/// Extractive summarization: select top-K sentences by word-frequency scoring
fn extractive_summary(text: &str, sentences: &[String], top_k: usize) -> Vec<String> {
    if sentences.is_empty() || top_k == 0 {
        return Vec::new();
    }

    // Build word frequency map from entire document
    let all_words = extract_words(text);
    let mut word_freq: BTreeMap<String, u32> = BTreeMap::new();
    for w in &all_words {
        if w.len() >= 3 && !STOP_WORDS.contains(&w.as_str()) {
            *word_freq.entry(w.clone()).or_insert(0) += 1;
        }
    }

    // Score each sentence by sum of word frequencies
    let mut scored: Vec<(usize, f32)> = Vec::new();
    for (idx, sentence) in sentences.iter().enumerate() {
        let words = extract_words(sentence);
        if words.is_empty() {
            scored.push((idx, 0.0));
            continue;
        }
        let score: f32 = words
            .iter()
            .map(|w| word_freq.get(w).copied().unwrap_or(0) as f32)
            .sum();
        // Normalize by sentence length to avoid bias toward long sentences
        let normalized = score / sqrt_f32(words.len() as f32);
        // Small penalty for very first sentence (often just a title) and very short sentences
        let penalty = if idx == 0 { 0.8 } else { 1.0 };
        let length_factor = if words.len() < 5 { 0.5 } else { 1.0 };
        scored.push((idx, normalized * penalty * length_factor));
    }

    // Sort by score descending
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));

    // Take top K indices, then sort by original order for coherence
    let mut top_indices: Vec<usize> = scored.iter().take(top_k).map(|(idx, _)| *idx).collect();
    top_indices.sort();

    // Collect sentences in original order
    top_indices
        .iter()
        .filter_map(|&idx| sentences.get(idx).cloned())
        .collect()
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static ANALYZER: Mutex<Option<DocumentAnalysis>> = Mutex::new(None);

pub fn init() {
    *ANALYZER.lock() = Some(DocumentAnalysis::new());
    crate::serial_println!(
        "    [document_analysis] Document analyzer ready (readability, keywords, summary)"
    );
}

/// Analyze a document from raw bytes
pub fn analyze(data: &[u8]) -> Result<DocumentAnalysis, ()> {
    let mut analysis = DocumentAnalysis::new();
    analysis.analyze(data)?;
    Ok(analysis)
}

/// Analyze a document from text
pub fn analyze_text(text: &str) -> DocumentAnalysis {
    let mut analysis = DocumentAnalysis::new();
    analysis.analyze_text(text);
    analysis
}

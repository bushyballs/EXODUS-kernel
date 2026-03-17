use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// General text and data classifier
///
/// Part of the Hoags AI subsystem. Multi-label classification
/// for routing documents, tagging content, and categorization.
///
/// Implements a Naive Bayes text classifier with Laplace smoothing.
/// Train with labeled examples, classify new text by computing
/// posterior probabilities per category using word frequencies.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// A classification label with confidence score
pub struct ClassLabel {
    pub label: String,
    pub confidence: f32,
}

/// Per-category statistics for Naive Bayes
struct CategoryStats {
    /// Total number of documents in this category
    doc_count: u64,
    /// Total number of word occurrences across all docs in this category
    total_words: u64,
    /// Word -> count within this category
    word_counts: BTreeMap<String, u64>,
}

impl CategoryStats {
    fn new() -> Self {
        CategoryStats {
            doc_count: 0,
            total_words: 0,
            word_counts: BTreeMap::new(),
        }
    }

    /// Add a document's words to this category
    fn add_document(&mut self, words: &[String]) {
        self.doc_count = self.doc_count.saturating_add(1);
        for w in words {
            let wc = self.word_counts.entry(w.clone()).or_insert(0);
            *wc = wc.saturating_add(1);
            self.total_words = self.total_words.saturating_add(1);
        }
    }

    /// Log probability of a word given this category (with Laplace smoothing)
    fn log_word_prob(&self, word: &str, vocab_size: u64) -> f64 {
        let count = self.word_counts.get(word).copied().unwrap_or(0);
        // Laplace smoothing: (count + 1) / (total_words + vocab_size)
        let numerator = (count + 1) as f64;
        let denominator = (self.total_words + vocab_size) as f64;
        ln_f64(numerator / denominator)
    }
}

/// Stop words to filter during feature extraction
const STOP_WORDS: &[&str] = &[
    "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "above", "below", "between", "and", "but", "or", "nor", "not", "no", "so",
    "if", "then", "than", "that", "this", "these", "those", "it", "its", "i", "me", "my", "we",
    "our", "you", "your", "he", "him", "his", "she", "her", "they", "them", "their",
];

pub struct Classifier {
    pub labels: Vec<String>,
    categories: BTreeMap<String, CategoryStats>,
    /// Total unique words seen across all categories
    vocab: BTreeMap<String, bool>,
    total_docs: u64,
}

impl Classifier {
    pub fn new() -> Self {
        Classifier {
            labels: Vec::new(),
            categories: BTreeMap::new(),
            vocab: BTreeMap::new(),
            total_docs: 0,
        }
    }

    /// Extract word features from text: lowercase, split on whitespace,
    /// filter stop words and short tokens, basic stemming
    fn extract_features(text: &str) -> Vec<String> {
        let mut words = Vec::new();
        for chunk in text.split(|c: char| !c.is_alphanumeric()) {
            if chunk.is_empty() {
                continue;
            }
            let lower = to_lowercase(chunk);
            if lower.len() < 2 {
                continue;
            }
            if STOP_WORDS.contains(&lower.as_str()) {
                continue;
            }
            // Simple suffix stemming
            let stemmed = simple_stem(&lower);
            words.push(stemmed);
        }
        words
    }

    /// Train the classifier with a labeled example
    pub fn train(&mut self, label: &str, text: &str) {
        let words = Self::extract_features(text);

        // Register label if new
        if !self.labels.iter().any(|l| l == label) {
            self.labels.push(String::from(label));
        }

        // Update vocabulary
        for w in &words {
            self.vocab.insert(w.clone(), true);
        }

        // Update category stats
        let cat = self
            .categories
            .entry(String::from(label))
            .or_insert_with(CategoryStats::new);
        cat.add_document(&words);
        self.total_docs = self.total_docs.saturating_add(1);
    }

    /// Train with multiple labeled examples at once
    pub fn train_batch(&mut self, examples: &[(&str, &str)]) {
        for &(label, text) in examples {
            self.train(label, text);
        }
    }

    /// Classify the input text, returning ranked labels with confidence scores.
    ///
    /// Uses Naive Bayes: P(category|text) ~ P(category) * product(P(word|category))
    /// Computed in log-space to avoid underflow.
    pub fn classify(&self, text: &str) -> Vec<ClassLabel> {
        if self.total_docs == 0 || self.categories.is_empty() {
            return Vec::new();
        }

        let words = Self::extract_features(text);
        if words.is_empty() {
            // No usable features; return uniform distribution
            let uniform = 1.0 / self.labels.len() as f32;
            return self
                .labels
                .iter()
                .map(|l| ClassLabel {
                    label: l.clone(),
                    confidence: uniform,
                })
                .collect();
        }

        let vocab_size = self.vocab.len() as u64;
        let total_docs_f = self.total_docs as f64;
        let mut log_probs: Vec<(String, f64)> = Vec::new();

        for (label, cat) in &self.categories {
            // Prior: log P(category)
            let log_prior = ln_f64(cat.doc_count as f64 / total_docs_f);

            // Likelihood: sum of log P(word|category)
            let mut log_likelihood = 0.0f64;
            for w in &words {
                log_likelihood += cat.log_word_prob(w, vocab_size);
            }

            log_probs.push((label.clone(), log_prior + log_likelihood));
        }

        // Convert log-probabilities to probabilities using log-sum-exp trick
        let max_log = log_probs
            .iter()
            .map(|(_, lp)| *lp)
            .fold(f64::NEG_INFINITY, |a, b| if b > a { b } else { a });

        let mut exp_probs: Vec<(String, f64)> = Vec::new();
        let mut sum_exp = 0.0f64;
        for (label, lp) in &log_probs {
            let e = exp_f64(*lp - max_log);
            sum_exp += e;
            exp_probs.push((label.clone(), e));
        }

        // Normalize to [0, 1] and build result
        let mut results: Vec<ClassLabel> = exp_probs
            .iter()
            .map(|(label, e)| {
                let confidence = if sum_exp > 0.0 {
                    (*e / sum_exp) as f32
                } else {
                    0.0
                };
                ClassLabel {
                    label: label.clone(),
                    confidence,
                }
            })
            .collect();

        // Sort by confidence descending
        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });

        results
    }

    /// Get the single best label for a text
    pub fn classify_best(&self, text: &str) -> Option<ClassLabel> {
        let results = self.classify(text);
        results.into_iter().next()
    }

    /// Number of training documents
    pub fn training_count(&self) -> u64 {
        self.total_docs
    }

    /// Number of unique words in vocabulary
    pub fn vocab_size(&self) -> usize {
        self.vocab.len()
    }

    /// Number of categories
    pub fn category_count(&self) -> usize {
        self.categories.len()
    }

    /// Get the document count for a specific category
    pub fn category_doc_count(&self, label: &str) -> u64 {
        self.categories.get(label).map(|c| c.doc_count).unwrap_or(0)
    }

    /// Get the top N most informative words for a category
    pub fn top_words(&self, label: &str, n: usize) -> Vec<(String, u64)> {
        match self.categories.get(label) {
            Some(cat) => {
                let mut words: Vec<(String, u64)> = cat
                    .word_counts
                    .iter()
                    .map(|(w, c)| (w.clone(), *c))
                    .collect();
                words.sort_by(|a, b| b.1.cmp(&a.1));
                words.truncate(n);
                words
            }
            None => Vec::new(),
        }
    }

    /// Classify multiple texts in one call.
    ///
    /// Returns a `Vec` of result vecs, one per input text. This avoids the
    /// overhead of acquiring the global lock repeatedly when classifying a
    /// batch of documents.
    pub fn classify_batch<'a>(&self, texts: &[&'a str]) -> Vec<Vec<ClassLabel>> {
        texts.iter().map(|t| self.classify(t)).collect()
    }

    /// Classify multiple texts and return only the best label per text.
    ///
    /// Returns `None` for any text that could not be classified (e.g., empty
    /// input or no training data).
    pub fn classify_batch_best<'a>(&self, texts: &[&'a str]) -> Vec<Option<ClassLabel>> {
        texts.iter().map(|t| self.classify_best(t)).collect()
    }

    /// Reset the classifier, clearing all training data
    pub fn reset(&mut self) {
        self.labels.clear();
        self.categories.clear();
        self.vocab.clear();
        self.total_docs = 0;
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn to_lowercase(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            result.push((c as u8 + 32) as char);
        } else {
            result.push(c);
        }
    }
    result
}

/// Simple Porter-like stemming: strip common suffixes
fn simple_stem(word: &str) -> String {
    let w = word;
    if w.ends_with("ation") && w.len() > 6 {
        return String::from(&w[..w.len() - 5]);
    }
    if w.ends_with("ment") && w.len() > 5 {
        return String::from(&w[..w.len() - 4]);
    }
    if w.ends_with("ness") && w.len() > 5 {
        return String::from(&w[..w.len() - 4]);
    }
    if w.ends_with("ing") && w.len() > 5 {
        return String::from(&w[..w.len() - 3]);
    }
    if w.ends_with("ies") && w.len() > 4 {
        let mut base = String::from(&w[..w.len() - 3]);
        base.push('y');
        return base;
    }
    if w.ends_with("ed") && w.len() > 4 {
        return String::from(&w[..w.len() - 2]);
    }
    if w.ends_with("ly") && w.len() > 4 {
        return String::from(&w[..w.len() - 2]);
    }
    if w.ends_with("s") && !w.ends_with("ss") && w.len() > 3 {
        return String::from(&w[..w.len() - 1]);
    }
    String::from(w)
}

/// Natural logarithm approximation for no_std
/// Uses the identity ln(x) = 2 * atanh((x-1)/(x+1)) with series expansion
fn ln_f64(x: f64) -> f64 {
    if x <= 0.0 {
        return -1e30; // approximate -infinity
    }
    if x == 1.0 {
        return 0.0;
    }

    // Range reduction: x = m * 2^e, ln(x) = ln(m) + e*ln(2)
    let mut val = x;
    let mut exp = 0i32;
    while val > 2.0 {
        val /= 2.0;
        exp += 1;
    }
    while val < 0.5 {
        val *= 2.0;
        exp -= 1;
    }

    // Now val is in [0.5, 2.0], compute ln(val) via atanh series
    let y = (val - 1.0) / (val + 1.0);
    let y2 = y * y;
    // ln(val) = 2 * (y + y^3/3 + y^5/5 + y^7/7 + ...)
    let mut term = y;
    let mut sum = y;
    for k in 1..20 {
        term *= y2;
        sum += term / (2 * k + 1) as f64;
    }
    sum *= 2.0;

    const LN2: f64 = 0.6931471805599453;
    sum + exp as f64 * LN2
}

/// Exponential function approximation for no_std
fn exp_f64(x: f64) -> f64 {
    if x > 700.0 {
        return 1e300;
    }
    if x < -700.0 {
        return 0.0;
    }

    // exp(x) = exp(n*ln2 + r) = 2^n * exp(r), where r in [-ln2/2, ln2/2]
    const LN2: f64 = 0.6931471805599453;
    let raw = x / LN2;
    let n = if raw >= 0.0 {
        (raw + 0.5) as i32
    } else {
        (raw - 0.5) as i32
    };
    let r = x - n as f64 * LN2;

    // Taylor series for exp(r)
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    for i in 1..25 {
        term *= r / i as f64;
        sum += term;
        if term.abs() < 1e-15 {
            break;
        }
    }

    // Multiply by 2^n
    let mut result = sum;
    if n > 0 {
        for _ in 0..n {
            result *= 2.0;
        }
    } else if n < 0 {
        let neg = (-n) as u32;
        for _ in 0..neg {
            result /= 2.0;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static CLASSIFIER: Mutex<Option<Classifier>> = Mutex::new(None);

pub fn init() {
    let mut clf = Classifier::new();

    // Pre-train with some baseline examples for common OS-level categories
    clf.train("system", "kernel panic crash error memory fault segfault");
    clf.train(
        "system",
        "boot loader initialization startup hardware driver",
    );
    clf.train("system", "process thread scheduler context switch priority");
    clf.train("system", "interrupt handler IRQ timer tick clock cycle");

    clf.train("network", "socket connection TCP UDP HTTP request response");
    clf.train(
        "network",
        "packet routing firewall DNS domain name resolution",
    );
    clf.train(
        "network",
        "bandwidth latency throughput download upload speed",
    );
    clf.train(
        "network",
        "wifi ethernet bluetooth wireless adapter interface",
    );

    clf.train("security", "password authentication login credential token");
    clf.train("security", "encryption key certificate SSL TLS cipher hash");
    clf.train(
        "security",
        "malware virus trojan ransomware exploit vulnerability",
    );
    clf.train(
        "security",
        "permission access control policy firewall intrusion",
    );

    clf.train("storage", "disk drive partition filesystem mount volume");
    clf.train(
        "storage",
        "file directory folder path read write create delete",
    );
    clf.train("storage", "SSD NVMe SATA backup archive compression");
    clf.train("storage", "database index query table record cache buffer");

    clf.train("user", "interface window button menu dialog click scroll");
    clf.train(
        "user",
        "application program software install update settings",
    );
    clf.train("user", "notification alert message popup toast reminder");
    clf.train(
        "user",
        "preference theme font color layout display resolution",
    );

    *CLASSIFIER.lock() = Some(clf);
    crate::serial_println!(
        "    [classifier] Naive Bayes text classifier ready (5 categories, 20 training docs)"
    );
}

/// Classify text using the global classifier
pub fn classify(text: &str) -> Vec<ClassLabel> {
    CLASSIFIER
        .lock()
        .as_ref()
        .map(|c| c.classify(text))
        .unwrap_or_else(Vec::new)
}

/// Train the global classifier with a new example
pub fn train(label: &str, text: &str) {
    if let Some(c) = CLASSIFIER.lock().as_mut() {
        c.train(label, text);
    }
}

/// Classify a batch of texts using the global classifier.
/// Returns one result vec per input text.
pub fn classify_batch(texts: &[&str]) -> Vec<Vec<ClassLabel>> {
    CLASSIFIER
        .lock()
        .as_ref()
        .map(|c| c.classify_batch(texts))
        .unwrap_or_else(|| texts.iter().map(|_| Vec::new()).collect())
}

/// Classify a batch and return only the best label per text.
pub fn classify_batch_best(texts: &[&str]) -> Vec<Option<ClassLabel>> {
    CLASSIFIER
        .lock()
        .as_ref()
        .map(|c| c.classify_batch_best(texts))
        .unwrap_or_else(|| texts.iter().map(|_| None).collect())
}

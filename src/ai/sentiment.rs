use crate::sync::Mutex;
use alloc::collections::BTreeMap;
/// Sentiment analysis engine
///
/// Part of the Hoags AI subsystem. Rule-based sentiment analyzer
/// with positive/negative word lexicons, negation handling,
/// intensity modifiers (very, extremely), and compound score calculation.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Sentiment classification result
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    Positive,
    Negative,
    Neutral,
}

/// Detailed sentiment analysis result
pub struct SentimentResult {
    pub sentiment: Sentiment,
    pub confidence: f32,
    /// Compound score in [-1.0, 1.0]
    pub compound_score: f32,
    /// Positive score component [0.0, 1.0]
    pub positive_score: f32,
    /// Negative score component [0.0, 1.0]
    pub negative_score: f32,
    /// Neutral score component [0.0, 1.0]
    pub neutral_score: f32,
    /// Words that contributed positively
    pub positive_words: Vec<String>,
    /// Words that contributed negatively
    pub negative_words: Vec<String>,
    /// Number of words analyzed
    pub word_count: usize,
}

/// Lexicon entry for a sentiment word
struct LexiconEntry {
    word: String,
    score: f32,
}

pub struct SentimentAnalyzer {
    pub threshold: f32,
    /// Positive word lexicon: word -> positive score
    positive_lexicon: Vec<LexiconEntry>,
    /// Negative word lexicon: word -> negative score (stored as negative)
    negative_lexicon: Vec<LexiconEntry>,
    /// Negation words that flip sentiment
    negation_words: Vec<String>,
    /// Intensity modifiers: word -> multiplier
    intensifiers: BTreeMap<String, f32>,
    /// Diminishers: word -> multiplier (< 1.0)
    diminishers: BTreeMap<String, f32>,
    /// Contrastive conjunctions: "but" shifts focus to latter clause
    contrast_words: Vec<String>,
    /// Total analyses performed
    total_analyses: u64,
}

impl SentimentAnalyzer {
    pub fn new() -> Self {
        let mut analyzer = SentimentAnalyzer {
            threshold: 0.15,
            positive_lexicon: Vec::new(),
            negative_lexicon: Vec::new(),
            negation_words: Vec::new(),
            intensifiers: BTreeMap::new(),
            diminishers: BTreeMap::new(),
            contrast_words: Vec::new(),
            total_analyses: 0,
        };
        analyzer.load_defaults();
        analyzer
    }

    /// Create with custom threshold
    pub fn with_threshold(threshold: f32) -> Self {
        let mut analyzer = Self::new();
        analyzer.threshold = threshold;
        analyzer
    }

    /// Load default lexicons
    fn load_defaults(&mut self) {
        // Positive words with scores
        let positives = [
            ("good", 0.6),
            ("great", 0.8),
            ("excellent", 0.9),
            ("amazing", 0.9),
            ("wonderful", 0.9),
            ("fantastic", 0.9),
            ("love", 0.8),
            ("best", 0.9),
            ("happy", 0.7),
            ("perfect", 1.0),
            ("awesome", 0.8),
            ("beautiful", 0.7),
            ("brilliant", 0.8),
            ("superb", 0.9),
            ("nice", 0.5),
            ("well", 0.3),
            ("fine", 0.3),
            ("pleased", 0.6),
            ("delighted", 0.8),
            ("enjoy", 0.6),
            ("impressive", 0.7),
            ("outstanding", 0.9),
            ("remarkable", 0.7),
            ("success", 0.7),
            ("win", 0.6),
            ("helpful", 0.6),
            ("like", 0.4),
            ("right", 0.3),
            ("fast", 0.4),
            ("clean", 0.4),
            ("smooth", 0.5),
            ("reliable", 0.6),
            ("stable", 0.5),
            ("safe", 0.5),
            ("easy", 0.4),
            ("cool", 0.5),
            ("fun", 0.6),
            ("exciting", 0.7),
            ("incredible", 0.8),
            ("marvelous", 0.8),
            ("terrific", 0.8),
            ("splendid", 0.8),
            ("glorious", 0.7),
            ("joyful", 0.7),
            ("cheerful", 0.6),
            ("grateful", 0.6),
            ("thankful", 0.6),
            ("fortunate", 0.6),
            ("lucky", 0.5),
            ("bright", 0.4),
            ("warm", 0.4),
            ("friendly", 0.5),
            ("kind", 0.5),
            ("generous", 0.6),
            ("brave", 0.5),
            ("elegant", 0.5),
            ("graceful", 0.5),
            ("charming", 0.5),
            ("delicious", 0.6),
            ("fresh", 0.4),
            ("productive", 0.5),
            ("efficient", 0.5),
            ("effective", 0.5),
            ("innovative", 0.6),
            ("creative", 0.5),
            ("smart", 0.5),
            ("clever", 0.5),
            ("wise", 0.5),
            ("recommend", 0.6),
            ("worth", 0.5),
            ("valuable", 0.6),
            ("important", 0.4),
            ("positive", 0.5),
            ("progress", 0.5),
            ("improve", 0.5),
        ];
        for (word, score) in &positives {
            self.positive_lexicon.push(LexiconEntry {
                word: String::from(*word),
                score: *score,
            });
        }

        // Negative words with scores (stored as negative)
        let negatives = [
            ("bad", -0.6),
            ("terrible", -0.9),
            ("awful", -0.9),
            ("horrible", -0.9),
            ("worst", -1.0),
            ("hate", -0.8),
            ("ugly", -0.6),
            ("broken", -0.7),
            ("error", -0.5),
            ("fail", -0.7),
            ("crash", -0.8),
            ("bug", -0.5),
            ("slow", -0.4),
            ("wrong", -0.6),
            ("problem", -0.5),
            ("issue", -0.4),
            ("poor", -0.6),
            ("annoying", -0.6),
            ("frustrating", -0.7),
            ("disappointed", -0.7),
            ("useless", -0.8),
            ("pathetic", -0.8),
            ("painful", -0.6),
            ("difficult", -0.3),
            ("confusing", -0.5),
            ("dangerous", -0.6),
            ("unstable", -0.5),
            ("laggy", -0.5),
            ("lose", -0.5),
            ("reject", -0.5),
            ("deny", -0.4),
            ("miss", -0.3),
            ("unfortunately", -0.4),
            ("sadly", -0.4),
            ("worse", -0.7),
            ("boring", -0.5),
            ("dull", -0.4),
            ("ugly", -0.5),
            ("nasty", -0.6),
            ("disgusting", -0.8),
            ("mediocre", -0.4),
            ("inferior", -0.5),
            ("weak", -0.4),
            ("clumsy", -0.4),
            ("sloppy", -0.5),
            ("messy", -0.4),
            ("waste", -0.5),
            ("garbage", -0.6),
            ("trash", -0.6),
            ("junk", -0.5),
            ("rubbish", -0.6),
            ("disaster", -0.8),
            ("catastrophe", -0.9),
            ("nightmare", -0.7),
            ("miserable", -0.7),
            ("depressing", -0.6),
            ("angry", -0.6),
            ("furious", -0.8),
            ("irritating", -0.6),
            ("annoyed", -0.5),
            ("upset", -0.5),
            ("worried", -0.4),
            ("afraid", -0.5),
            ("scary", -0.5),
            ("fear", -0.5),
            ("grief", -0.6),
            ("sorrow", -0.6),
            ("regret", -0.5),
            ("shame", -0.5),
            ("guilt", -0.5),
            ("hostile", -0.6),
            ("cruel", -0.7),
            ("wicked", -0.6),
            ("evil", -0.7),
            ("toxic", -0.6),
            ("negative", -0.4),
            ("harm", -0.6),
            ("damage", -0.5),
            ("destroy", -0.7),
        ];
        for (word, score) in &negatives {
            self.negative_lexicon.push(LexiconEntry {
                word: String::from(*word),
                score: *score,
            });
        }

        // Negation words
        self.negation_words = [
            "not", "no", "never", "neither", "nobody", "nothing", "nowhere", "nor", "cannot",
            "hardly", "barely", "scarcely", "seldom", "rarely", "without",
        ]
        .iter()
        .map(|s| String::from(*s))
        .collect();

        // Intensifiers (multiply the sentiment score)
        let intensifiers = [
            ("very", 1.5),
            ("really", 1.4),
            ("extremely", 1.8),
            ("incredibly", 1.7),
            ("absolutely", 1.6),
            ("totally", 1.5),
            ("completely", 1.5),
            ("utterly", 1.7),
            ("remarkably", 1.5),
            ("exceptionally", 1.6),
            ("particularly", 1.3),
            ("especially", 1.3),
            ("immensely", 1.6),
            ("tremendously", 1.6),
            ("enormously", 1.5),
            ("highly", 1.4),
            ("deeply", 1.4),
            ("intensely", 1.5),
            ("profoundly", 1.5),
            ("genuinely", 1.2),
            ("truly", 1.3),
            ("most", 1.3),
            ("so", 1.3),
        ];
        for (word, mult) in &intensifiers {
            self.intensifiers.insert(String::from(*word), *mult);
        }

        // Diminishers (reduce the sentiment score)
        let diminishers = [
            ("somewhat", 0.7),
            ("slightly", 0.6),
            ("barely", 0.4),
            ("hardly", 0.4),
            ("fairly", 0.8),
            ("quite", 0.9),
            ("sort of", 0.6),
            ("kind of", 0.6),
            ("a bit", 0.5),
            ("a little", 0.5),
            ("mildly", 0.6),
            ("marginally", 0.5),
        ];
        for (word, mult) in &diminishers {
            self.diminishers.insert(String::from(*word), *mult);
        }

        // Contrastive conjunctions
        self.contrast_words = [
            "but",
            "however",
            "although",
            "though",
            "yet",
            "nevertheless",
            "nonetheless",
            "despite",
            "in spite of",
        ]
        .iter()
        .map(|s| String::from(*s))
        .collect();
    }

    /// Add a custom positive word
    pub fn add_positive(&mut self, word: &str, score: f32) {
        self.positive_lexicon.push(LexiconEntry {
            word: String::from(word),
            score: score.max(0.0).min(1.0),
        });
    }

    /// Add a custom negative word
    pub fn add_negative(&mut self, word: &str, score: f32) {
        self.negative_lexicon.push(LexiconEntry {
            word: String::from(word),
            score: -(score.max(0.0).min(1.0)),
        });
    }

    /// Classify the sentiment of the given text (simple API)
    pub fn analyze(&self, text: &str) -> (Sentiment, f32) {
        let result = self.analyze_detailed(text);
        (result.sentiment, result.confidence)
    }

    /// Detailed sentiment analysis
    pub fn analyze_detailed(&self, text: &str) -> SentimentResult {
        let words = tokenize(text);
        if words.is_empty() {
            return SentimentResult {
                sentiment: Sentiment::Neutral,
                confidence: 1.0,
                compound_score: 0.0,
                positive_score: 0.0,
                negative_score: 0.0,
                neutral_score: 1.0,
                positive_words: Vec::new(),
                negative_words: Vec::new(),
                word_count: 0,
            };
        }

        let mut sentiment_scores: Vec<f32> = Vec::new();
        let mut positive_words = Vec::new();
        let mut negative_words = Vec::new();

        // Process each word with context awareness
        for (i, word) in words.iter().enumerate() {
            let lower = word.to_lowercase();

            // Look up in lexicons
            let mut word_score = 0.0f32;
            let mut found = false;

            for entry in &self.positive_lexicon {
                if entry.word == lower {
                    word_score = entry.score;
                    found = true;
                    break;
                }
            }
            if !found {
                for entry in &self.negative_lexicon {
                    if entry.word == lower {
                        word_score = entry.score;
                        found = true;
                        break;
                    }
                }
            }

            // Handle contraction negation ("n't")
            if lower.ends_with("n't") || lower.ends_with("nt") {
                // This word itself is a negation modifier
                // Check the next word
                continue;
            }

            if !found {
                continue;
            }

            // Check for preceding negation (within 3 words)
            let negation_window = if i >= 3 { i - 3 } else { 0 };
            let mut negated = false;
            for j in negation_window..i {
                let prev = words[j].to_lowercase();
                if self.negation_words.iter().any(|n| n == &prev) {
                    negated = true;
                    break;
                }
                // Contraction negation
                if prev.ends_with("n't") || prev.ends_with("nt") {
                    negated = true;
                    break;
                }
            }

            if negated {
                word_score = -word_score * 0.75; // Flip and dampen
            }

            // Check for preceding intensifier
            if i > 0 {
                let prev = words[i - 1].to_lowercase();
                if let Some(&mult) = self.intensifiers.get(&prev) {
                    word_score *= mult;
                } else if let Some(&mult) = self.diminishers.get(&prev) {
                    word_score *= mult;
                }

                // Two-word intensifiers: "very much", check i-2
                if i > 1 {
                    let prev2 = words[i - 2].to_lowercase();
                    if let Some(&mult) = self.intensifiers.get(&prev2) {
                        if prev == "much" || prev == "more" || prev == "most" {
                            word_score *= mult;
                        }
                    }
                }
            }

            // Track contributing words
            if word_score > 0.0 {
                positive_words.push(lower.clone());
            } else if word_score < 0.0 {
                negative_words.push(lower.clone());
            }

            sentiment_scores.push(word_score);
        }

        // Handle contrastive conjunctions: weight later clause more heavily
        let has_contrast = words
            .iter()
            .any(|w| self.contrast_words.iter().any(|c| c == &w.to_lowercase()));

        // Compute compound score
        let compound = if sentiment_scores.is_empty() {
            0.0
        } else if has_contrast {
            // Find the contrast word position
            let contrast_pos = words
                .iter()
                .position(|w| self.contrast_words.iter().any(|c| c == &w.to_lowercase()))
                .unwrap_or(words.len() / 2);

            // Weight the latter part more heavily
            let mut before_sum = 0.0f32;
            let mut after_sum = 0.0f32;
            let mut score_idx = 0;
            for (i, word) in words.iter().enumerate() {
                let lower = word.to_lowercase();
                let has_score = self.positive_lexicon.iter().any(|e| e.word == lower)
                    || self.negative_lexicon.iter().any(|e| e.word == lower);
                if has_score && score_idx < sentiment_scores.len() {
                    if i < contrast_pos {
                        before_sum += sentiment_scores[score_idx];
                    } else {
                        after_sum += sentiment_scores[score_idx];
                    }
                    score_idx += 1;
                }
            }
            // Latter clause gets 70% weight, former 30%
            0.3 * before_sum + 0.7 * after_sum
        } else {
            sentiment_scores.iter().sum::<f32>()
        };

        // Normalize compound to [-1.0, 1.0] using a sigmoid-like function
        let normalized = normalize_score(compound, sentiment_scores.len());

        // Calculate positive/negative/neutral proportions
        let pos_total: f32 = sentiment_scores.iter().filter(|&&s| s > 0.0).sum();
        let neg_total: f32 = sentiment_scores
            .iter()
            .filter(|&&s| s < 0.0)
            .map(|s| s.abs())
            .sum();
        let abs_total = pos_total + neg_total;

        let (pos_pct, neg_pct, neu_pct) = if abs_total < 0.01 {
            (0.0, 0.0, 1.0)
        } else {
            let neutral_count = words.len() - sentiment_scores.len();
            let _total = words.len() as f32;
            let p = pos_total / (abs_total + neutral_count as f32);
            let n = neg_total / (abs_total + neutral_count as f32);
            let neu = 1.0 - p - n;
            (p, n, neu.max(0.0))
        };

        // Determine sentiment class
        let sentiment = if normalized > self.threshold {
            Sentiment::Positive
        } else if normalized < -self.threshold {
            Sentiment::Negative
        } else {
            Sentiment::Neutral
        };

        // Confidence: how far from the threshold boundary
        let confidence = if sentiment == Sentiment::Neutral {
            1.0 - (normalized.abs() / self.threshold).min(1.0)
        } else {
            ((normalized.abs() - self.threshold) / (1.0 - self.threshold)).min(1.0)
        };

        SentimentResult {
            sentiment,
            confidence: confidence.max(0.0).min(1.0),
            compound_score: normalized,
            positive_score: pos_pct,
            negative_score: neg_pct,
            neutral_score: neu_pct,
            positive_words,
            negative_words,
            word_count: words.len(),
        }
    }

    /// Analyze sentiment of multiple texts, return aggregate
    pub fn analyze_batch(&self, texts: &[&str]) -> SentimentResult {
        if texts.is_empty() {
            return SentimentResult {
                sentiment: Sentiment::Neutral,
                confidence: 1.0,
                compound_score: 0.0,
                positive_score: 0.0,
                negative_score: 0.0,
                neutral_score: 1.0,
                positive_words: Vec::new(),
                negative_words: Vec::new(),
                word_count: 0,
            };
        }

        let mut total_compound = 0.0f32;
        let mut total_pos = 0.0f32;
        let mut total_neg = 0.0f32;
        let mut total_neu = 0.0f32;
        let mut all_pos_words = Vec::new();
        let mut all_neg_words = Vec::new();
        let mut total_words = 0;

        for text in texts {
            let result = self.analyze_detailed(text);
            total_compound += result.compound_score;
            total_pos += result.positive_score;
            total_neg += result.negative_score;
            total_neu += result.neutral_score;
            all_pos_words.extend(result.positive_words);
            all_neg_words.extend(result.negative_words);
            total_words += result.word_count;
        }

        let n = texts.len() as f32;
        let avg_compound = total_compound / n;

        let sentiment = if avg_compound > self.threshold {
            Sentiment::Positive
        } else if avg_compound < -self.threshold {
            Sentiment::Negative
        } else {
            Sentiment::Neutral
        };

        SentimentResult {
            sentiment,
            confidence: avg_compound.abs().min(1.0),
            compound_score: avg_compound,
            positive_score: total_pos / n,
            negative_score: total_neg / n,
            neutral_score: total_neu / n,
            positive_words: all_pos_words,
            negative_words: all_neg_words,
            word_count: total_words,
        }
    }

    /// Get vocabulary size
    pub fn vocab_size(&self) -> usize {
        self.positive_lexicon.len() + self.negative_lexicon.len()
    }

    /// Total analyses performed
    pub fn total_analyses(&self) -> u64 {
        self.total_analyses
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Tokenize text into words
fn tokenize(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();

    for c in text.chars() {
        if c.is_alphanumeric() || c == '\'' || c == '-' {
            current.push(c);
        } else {
            if !current.is_empty() {
                words.push(core::mem::replace(&mut current, String::new()));
            }
            // Punctuation as sentiment signal
            if c == '!' {
                // Exclamation marks intensify
                if let Some(last) = words.last_mut() {
                    last.push('!');
                }
            }
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Normalize a raw sentiment sum to [-1.0, 1.0]
/// Uses: score / sqrt(score^2 + alpha) where alpha scales with count
fn normalize_score(score: f32, count: usize) -> f32 {
    let alpha = 15.0 + count as f32 * 0.5; // Adaptive normalization
    score / sqrt_f32(score * score + alpha)
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

static ANALYZER: Mutex<Option<SentimentAnalyzer>> = Mutex::new(None);

pub fn init() {
    let analyzer = SentimentAnalyzer::new();
    let vocab = analyzer.vocab_size();
    *ANALYZER.lock() = Some(analyzer);
    crate::serial_println!(
        "    [sentiment] Sentiment analyzer ready ({} lexicon entries, negation+intensifiers)",
        vocab
    );
}

/// Analyze sentiment of text
pub fn analyze(text: &str) -> (Sentiment, f32) {
    ANALYZER
        .lock()
        .as_ref()
        .map(|a| a.analyze(text))
        .unwrap_or((Sentiment::Neutral, 0.0))
}

/// Detailed sentiment analysis
pub fn analyze_detailed(text: &str) -> Option<SentimentResult> {
    ANALYZER.lock().as_ref().map(|a| a.analyze_detailed(text))
}

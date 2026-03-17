/// AI-powered language for Genesis
///
/// Smart translation, context-aware input prediction,
/// language detection, writing assistance, grammar correction.
///
/// Inspired by: Google Translate on-device, Apple Translate. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// Language detection result
pub struct LanguageDetection {
    pub language: String,
    pub confidence: f32,
    pub script: String,
}

/// Input prediction
pub struct InputPrediction {
    pub word: String,
    pub confidence: f32,
    pub is_emoji: bool,
}

/// Grammar correction
pub struct GrammarCorrection {
    pub original: String,
    pub corrected: String,
    pub rule: String,
    pub confidence: f32,
}

/// Writing suggestion
pub struct WritingSuggestion {
    pub suggestion_type: WritingSuggestionType,
    pub text: String,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritingSuggestionType {
    WordCompletion,
    NextWord,
    PhraseCompletion,
    Emoji,
    GrammarFix,
    SpellingFix,
    ToneAdjust,
}

/// AI language engine
pub struct AiLanguageEngine {
    pub enabled: bool,
    pub user_dictionary: BTreeMap<String, u32>,
    pub word_frequency: BTreeMap<String, u32>,
    pub bigrams: BTreeMap<(String, String), u32>,
    pub corrections_applied: u64,
    pub predictions_accepted: u64,
    pub predictions_shown: u64,
    pub preferred_language: String,
    pub auto_correct: bool,
    pub predictive_text: bool,
    pub swipe_typing: bool,
}

impl AiLanguageEngine {
    const fn new() -> Self {
        AiLanguageEngine {
            enabled: true,
            user_dictionary: BTreeMap::new(),
            word_frequency: BTreeMap::new(),
            bigrams: BTreeMap::new(),
            corrections_applied: 0,
            predictions_accepted: 0,
            predictions_shown: 0,
            preferred_language: String::new(),
            auto_correct: true,
            predictive_text: true,
            swipe_typing: true,
        }
    }

    /// Detect language of text
    pub fn detect_language(&self, text: &str) -> LanguageDetection {
        let mut scores: BTreeMap<&str, f32> = BTreeMap::new();

        // Character-based heuristics
        for ch in text.chars() {
            match ch {
                '\u{0400}'..='\u{04FF}' => *scores.entry("ru").or_insert(0.0) += 1.0,
                '\u{4E00}'..='\u{9FFF}' => *scores.entry("zh").or_insert(0.0) += 1.0,
                '\u{3040}'..='\u{309F}' => *scores.entry("ja").or_insert(0.0) += 1.0,
                '\u{AC00}'..='\u{D7AF}' => *scores.entry("ko").or_insert(0.0) += 1.0,
                '\u{0600}'..='\u{06FF}' => *scores.entry("ar").or_insert(0.0) += 1.0,
                '\u{0900}'..='\u{097F}' => *scores.entry("hi").or_insert(0.0) += 1.0,
                'a'..='z' | 'A'..='Z' => *scores.entry("en").or_insert(0.0) += 0.5,
                _ => {}
            }
        }

        // Common word detection for Latin-script languages
        let lower = text.to_lowercase();
        let en_words = ["the", "is", "and", "to", "of", "in", "it", "for"];
        let es_words = ["el", "la", "de", "en", "que", "los", "del", "por"];
        let fr_words = ["le", "la", "de", "et", "les", "des", "une", "est"];
        let de_words = ["der", "die", "und", "ist", "ein", "den", "das", "auf"];

        for word in lower.split_whitespace() {
            if en_words.contains(&word) {
                *scores.entry("en").or_insert(0.0) += 2.0;
            }
            if es_words.contains(&word) {
                *scores.entry("es").or_insert(0.0) += 2.0;
            }
            if fr_words.contains(&word) {
                *scores.entry("fr").or_insert(0.0) += 2.0;
            }
            if de_words.contains(&word) {
                *scores.entry("de").or_insert(0.0) += 2.0;
            }
        }

        let (lang, score) = scores
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(core::cmp::Ordering::Equal))
            .map(|(l, s)| (*l, *s))
            .unwrap_or(("en", 0.0));

        let total: f32 = scores.values().sum();
        let conf = if total > 0.0 { score / total } else { 0.0 };

        LanguageDetection {
            language: String::from(lang),
            confidence: conf.min(1.0),
            script: String::from(match lang {
                "zh" | "ja" => "CJK",
                "ko" => "Hangul",
                "ar" => "Arabic",
                "hi" => "Devanagari",
                "ru" => "Cyrillic",
                _ => "Latin",
            }),
        }
    }

    /// Predict next word based on context
    pub fn predict_next_word(&mut self, context: &str) -> Vec<InputPrediction> {
        self.predictions_shown = self.predictions_shown.saturating_add(1);
        let words: Vec<&str> = context.split_whitespace().collect();
        let last_word = words.last().copied().unwrap_or("");

        let mut predictions = Vec::new();

        // Check bigram model
        if !last_word.is_empty() {
            let last_lower = last_word.to_lowercase();
            for ((w1, w2), freq) in &self.bigrams {
                if w1.to_lowercase() == last_lower {
                    predictions.push(InputPrediction {
                        word: w2.clone(),
                        confidence: (*freq as f32 / 100.0).min(1.0),
                        is_emoji: false,
                    });
                }
            }
        }

        // Frequency-based fallback
        if predictions.is_empty() {
            let mut freq_list: Vec<(&String, &u32)> = self.word_frequency.iter().collect();
            freq_list.sort_by(|a, b| b.1.cmp(a.1));
            for (word, freq) in freq_list.iter().take(3) {
                predictions.push(InputPrediction {
                    word: (*word).clone(),
                    confidence: (**freq as f32 / 1000.0).min(1.0),
                    is_emoji: false,
                });
            }
        }

        predictions.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        predictions.truncate(3);
        predictions
    }

    /// Learn from typed text
    pub fn learn_text(&mut self, text: &str) {
        let words: Vec<&str> = text.split_whitespace().collect();
        for word in &words {
            let wf = self.word_frequency.entry(String::from(*word)).or_insert(0);
            *wf = wf.saturating_add(1);
        }
        for window in words.windows(2) {
            let key = (String::from(window[0]), String::from(window[1]));
            let bg = self.bigrams.entry(key).or_insert(0);
            *bg = bg.saturating_add(1);
        }
    }

    /// Check grammar (simple rules)
    pub fn check_grammar(&self, text: &str) -> Vec<GrammarCorrection> {
        let mut corrections = Vec::new();
        let words: Vec<&str> = text.split_whitespace().collect();

        for i in 0..words.len() {
            // Double word detection
            if i > 0 && words[i].to_lowercase() == words[i - 1].to_lowercase() {
                corrections.push(GrammarCorrection {
                    original: alloc::format!("{} {}", words[i - 1], words[i]),
                    corrected: String::from(words[i]),
                    rule: String::from("repeated_word"),
                    confidence: 0.95,
                });
            }

            // a/an rule
            if (words[i] == "a" || words[i] == "A") && i + 1 < words.len() {
                let next = words[i + 1].to_lowercase();
                let vowels = ['a', 'e', 'i', 'o', 'u'];
                if next.chars().next().map_or(false, |c| vowels.contains(&c)) {
                    corrections.push(GrammarCorrection {
                        original: alloc::format!("a {}", words[i + 1]),
                        corrected: alloc::format!("an {}", words[i + 1]),
                        rule: String::from("a_an"),
                        confidence: 0.85,
                    });
                }
            }
        }

        corrections
    }

    /// Add word to user dictionary
    pub fn add_to_dictionary(&mut self, word: &str) {
        let ud = self.user_dictionary.entry(String::from(word)).or_insert(0);
        *ud = ud.saturating_add(1);
    }
}

fn seed_common_words(engine: &mut AiLanguageEngine) {
    let common = [
        ("the", 100),
        ("is", 80),
        ("and", 90),
        ("to", 85),
        ("of", 75),
        ("in", 70),
        ("it", 65),
        ("for", 60),
        ("on", 55),
        ("with", 50),
        ("that", 48),
        ("was", 45),
        ("are", 42),
        ("be", 40),
        ("have", 38),
        ("not", 35),
        ("this", 33),
        ("but", 30),
        ("from", 28),
        ("or", 25),
    ];
    for (word, freq) in &common {
        engine.word_frequency.insert(String::from(*word), *freq);
    }

    let bigram_data = [
        ("I", "am"),
        ("I", "have"),
        ("I", "will"),
        ("it", "is"),
        ("do", "not"),
        ("can", "not"),
        ("this", "is"),
        ("that", "is"),
        ("would", "be"),
        ("have", "been"),
        ("will", "be"),
        ("should", "be"),
    ];
    for (w1, w2) in &bigram_data {
        engine
            .bigrams
            .insert((String::from(*w1), String::from(*w2)), 10);
    }
}

static AI_LANG: Mutex<AiLanguageEngine> = Mutex::new(AiLanguageEngine::new());

pub fn init() {
    seed_common_words(&mut AI_LANG.lock());
    crate::serial_println!(
        "    [ai-lang] AI language initialized (detect, predict, grammar, dictionary)"
    );
}

pub fn detect_language(text: &str) -> LanguageDetection {
    AI_LANG.lock().detect_language(text)
}

pub fn predict_next(context: &str) -> Vec<InputPrediction> {
    AI_LANG.lock().predict_next_word(context)
}

pub fn learn_text(text: &str) {
    AI_LANG.lock().learn_text(text);
}

pub fn check_grammar(text: &str) -> Vec<GrammarCorrection> {
    AI_LANG.lock().check_grammar(text)
}

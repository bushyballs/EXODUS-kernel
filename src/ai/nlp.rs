/// Natural Language Processing pipeline for Genesis
///
/// Tokenization, sentence segmentation, POS tagging,
/// named entity recognition, sentiment analysis, and
/// text classification — all running locally.
///
/// Inspired by: spaCy, NLTK, Hugging Face. All code is original.
use crate::sync::Mutex;
use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Part of speech tag
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosTag {
    Noun,
    Verb,
    Adjective,
    Adverb,
    Pronoun,
    Preposition,
    Conjunction,
    Determiner,
    Interjection,
    Number,
    Punctuation,
    Unknown,
}

/// Named entity type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    Person,
    Organization,
    Location,
    Date,
    Time,
    Money,
    Percent,
    Email,
    Url,
    PhoneNumber,
    FilePath,
    Command,
}

/// Sentiment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sentiment {
    VeryPositive,
    Positive,
    Neutral,
    Negative,
    VeryNegative,
}

/// A token in NLP pipeline
pub struct NlpToken {
    pub text: String,
    pub pos: PosTag,
    pub start: usize,
    pub end: usize,
    pub lemma: String,
    pub is_stop_word: bool,
}

/// A named entity
pub struct NamedEntity {
    pub text: String,
    pub entity_type: EntityType,
    pub start: usize,
    pub end: usize,
    pub confidence: f32,
}

/// Processed document
pub struct NlpDocument {
    pub text: String,
    pub tokens: Vec<NlpToken>,
    pub entities: Vec<NamedEntity>,
    pub sentences: Vec<(usize, usize)>, // start, end byte offsets
    pub sentiment: Sentiment,
    pub sentiment_score: f32,
    pub language: String,
    pub keywords: Vec<(String, f32)>,
}

/// NLP pipeline
pub struct NlpPipeline {
    pub stop_words: Vec<String>,
    pub entity_patterns: Vec<(String, EntityType)>,
    pub sentiment_words: BTreeMap<String, f32>,
}

impl NlpPipeline {
    const fn new() -> Self {
        NlpPipeline {
            stop_words: Vec::new(),
            entity_patterns: Vec::new(),
            sentiment_words: BTreeMap::new(),
        }
    }

    pub fn load_defaults(&mut self) {
        let stops = [
            "the", "a", "an", "is", "are", "was", "were", "be", "been", "being", "have", "has",
            "had", "do", "does", "did", "will", "would", "could", "should", "may", "might",
            "shall", "can", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
            "into", "through", "during", "before", "after", "above", "below", "between", "and",
            "but", "or", "nor", "not", "no", "so", "if", "then", "than", "that", "this", "these",
            "those", "it", "its",
        ];
        for w in &stops {
            self.stop_words.push(String::from(*w));
        }

        let positive = [
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
        ];
        for (w, score) in &positive {
            self.sentiment_words.insert(String::from(*w), *score);
        }

        let negative = [
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
        ];
        for (w, score) in &negative {
            self.sentiment_words.insert(String::from(*w), *score);
        }

        let intensifiers = [
            ("very", 1.5),
            ("really", 1.4),
            ("extremely", 1.8),
            ("incredibly", 1.7),
            ("absolutely", 1.6),
            ("totally", 1.5),
            ("completely", 1.5),
            ("utterly", 1.7),
            ("quite", 1.2),
            ("fairly", 1.1),
            ("somewhat", 0.8),
            ("slightly", 0.6),
            ("barely", 0.4),
            ("hardly", 0.4),
        ];
        for (w, _) in &intensifiers {
            self.entity_patterns
                .push((String::from(*w), EntityType::Command));
        }
    }

    /// Tokenize text with punctuation splitting and multi-word entity awareness
    pub fn process(&self, text: &str) -> NlpDocument {
        let tokens = self.tokenize(text);
        let sentences = self.split_sentences(text);
        let entities = self.recognize_entities(text, &tokens);
        let (sentiment, score) = self.analyze_sentiment(&tokens);
        let keywords = self.extract_keywords(&tokens);

        NlpDocument {
            text: String::from(text),
            tokens,
            entities,
            sentences,
            sentiment,
            sentiment_score: score,
            language: String::from("en"),
            keywords,
        }
    }

    /// Tokenize text, splitting punctuation into separate tokens
    fn tokenize(&self, text: &str) -> Vec<NlpToken> {
        let mut tokens = Vec::new();
        let mut raw_tokens = Vec::new();

        // Phase 1: split on whitespace, then split punctuation off each chunk
        let mut pos = 0;
        for segment in text.split_whitespace() {
            let seg_start = match text[pos..].find(segment) {
                Some(offset) => pos + offset,
                None => pos,
            };
            let seg_end = seg_start + segment.len();

            let sub_tokens = split_punctuation(segment, seg_start);
            raw_tokens.extend(sub_tokens);
            pos = seg_end;
        }

        // Phase 2: tag each raw token
        for (raw_text, start, end) in &raw_tokens {
            if raw_text.is_empty() {
                continue;
            }

            let first_char = match raw_text.chars().next() {
                Some(c) => c,
                None => continue,
            };

            // Pure punctuation token
            if raw_text.len() == 1 && first_char.is_ascii_punctuation() {
                tokens.push(NlpToken {
                    text: raw_text.clone(),
                    pos: PosTag::Punctuation,
                    start: *start,
                    end: *end,
                    lemma: raw_text.clone(),
                    is_stop_word: false,
                });
                continue;
            }

            let lower = raw_text.to_lowercase();
            let tag = self.guess_pos(&lower, first_char);
            let is_stop = self.stop_words.iter().any(|s| s == &lower);
            let lemma = simple_lemmatize(&lower);

            tokens.push(NlpToken {
                text: raw_text.clone(),
                pos: tag,
                start: *start,
                end: *end,
                lemma,
                is_stop_word: is_stop,
            });
        }
        tokens
    }

    /// Split text into sentences, handling abbreviations and multi-char endings
    fn split_sentences(&self, text: &str) -> Vec<(usize, usize)> {
        let mut sentences = Vec::new();
        let mut sent_start = 0;
        let bytes = text.as_bytes();
        let len = bytes.len();

        let mut i = 0;
        while i < len {
            let b = bytes[i];
            if b == b'.' || b == b'!' || b == b'?' {
                // Consume consecutive sentence-ending punctuation (e.g., "..." or "?!")
                let mut end = i + 1;
                while end < len && (bytes[end] == b'.' || bytes[end] == b'!' || bytes[end] == b'?')
                {
                    end += 1;
                }

                // Check if this is likely an abbreviation (single uppercase letter before dot)
                let is_abbrev = b == b'.'
                    && i > 0
                    && i + 1 < len
                    && bytes[i - 1].is_ascii_uppercase()
                    && i >= 2
                    && (bytes[i - 2] == b' ' || i == 1);

                if !is_abbrev {
                    sentences.push((sent_start, end));
                    // Skip whitespace after sentence
                    while end < len && bytes[end] == b' ' {
                        end += 1;
                    }
                    sent_start = end;
                }
                i = end;
            } else {
                i += 1;
            }
        }

        if sent_start < len {
            // Trim trailing whitespace from the last fragment
            let mut end = len;
            while end > sent_start && bytes[end - 1] == b' ' {
                end -= 1;
            }
            if end > sent_start {
                sentences.push((sent_start, end));
            }
        }
        sentences
    }

    fn guess_pos(&self, lower: &str, first_char: char) -> PosTag {
        // Numbers (integers, decimals, negatives)
        if is_number_like(lower) {
            return PosTag::Number;
        }

        // Pronouns
        let pronouns = [
            "i",
            "me",
            "my",
            "mine",
            "myself",
            "you",
            "your",
            "yours",
            "yourself",
            "he",
            "him",
            "his",
            "himself",
            "she",
            "her",
            "hers",
            "herself",
            "we",
            "us",
            "our",
            "ours",
            "ourselves",
            "they",
            "them",
            "their",
            "theirs",
            "themselves",
            "who",
            "whom",
            "whose",
            "which",
            "what",
        ];
        if pronouns.contains(&lower) {
            return PosTag::Pronoun;
        }

        // Determiners
        let determiners = [
            "the", "a", "an", "this", "that", "these", "those", "some", "any", "each", "every",
            "all", "both", "few", "several", "many", "much", "no", "neither", "either",
        ];
        if determiners.contains(&lower) {
            return PosTag::Determiner;
        }

        // Prepositions
        let prepositions = [
            "in", "on", "at", "to", "for", "with", "from", "by", "about", "above", "below",
            "between", "into", "through", "during", "before", "after", "over", "under", "around",
            "among", "against", "upon", "toward", "towards", "within", "without", "along",
            "across", "behind", "beside", "beyond", "near", "off", "onto", "outside", "past",
            "since", "until", "up", "down",
        ];
        if prepositions.contains(&lower) {
            return PosTag::Preposition;
        }

        // Conjunctions
        let conjunctions = [
            "and",
            "but",
            "or",
            "nor",
            "for",
            "yet",
            "so",
            "because",
            "although",
            "while",
            "whereas",
            "unless",
            "however",
            "therefore",
            "moreover",
            "furthermore",
            "nevertheless",
            "though",
        ];
        if conjunctions.contains(&lower) {
            return PosTag::Conjunction;
        }

        // Interjections
        let interjections = [
            "oh", "wow", "hey", "hi", "hello", "oops", "ouch", "ugh", "yay", "hmm", "huh", "whoa",
            "alas", "bravo",
        ];
        if interjections.contains(&lower) {
            return PosTag::Interjection;
        }

        // Common verbs (high frequency)
        let common_verbs = [
            "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does",
            "did", "will", "would", "could", "should", "may", "might", "shall", "can", "must",
            "go", "get", "make", "know", "think", "take", "see", "come", "want", "use", "find",
            "give", "tell", "work", "call", "try", "ask", "need", "feel", "become", "leave", "put",
            "mean", "keep", "let", "begin", "seem", "help", "show", "hear", "play", "run", "move",
            "live", "believe", "happen", "write", "read", "learn", "change", "follow", "stop",
            "start", "open", "close", "set", "say", "said",
        ];
        if common_verbs.contains(&lower) {
            return PosTag::Verb;
        }

        // Suffix-based rules (applied after dictionary lookups)
        if lower.ends_with("ing") {
            return PosTag::Verb;
        }
        if lower.ends_with("ed") && lower.len() > 3 {
            return PosTag::Verb;
        }
        if lower.ends_with("ize") || lower.ends_with("ise") {
            return PosTag::Verb;
        }
        if lower.ends_with("ate") && lower.len() > 4 {
            return PosTag::Verb;
        }
        if lower.ends_with("ify") {
            return PosTag::Verb;
        }

        if lower.ends_with("ly") && lower.len() > 3 {
            return PosTag::Adverb;
        }

        if lower.ends_with("tion") || lower.ends_with("sion") {
            return PosTag::Noun;
        }
        if lower.ends_with("ment") && lower.len() > 4 {
            return PosTag::Noun;
        }
        if lower.ends_with("ness") {
            return PosTag::Noun;
        }
        if lower.ends_with("ity") || lower.ends_with("ety") {
            return PosTag::Noun;
        }
        if lower.ends_with("ance") || lower.ends_with("ence") {
            return PosTag::Noun;
        }
        if lower.ends_with("ism") {
            return PosTag::Noun;
        }
        if lower.ends_with("ist") {
            return PosTag::Noun;
        }
        if lower.ends_with("er") && lower.len() > 3 {
            return PosTag::Noun;
        }
        if lower.ends_with("or") && lower.len() > 3 {
            return PosTag::Noun;
        }

        if lower.ends_with("ful") {
            return PosTag::Adjective;
        }
        if lower.ends_with("ous") {
            return PosTag::Adjective;
        }
        if lower.ends_with("ive") {
            return PosTag::Adjective;
        }
        if lower.ends_with("able") || lower.ends_with("ible") {
            return PosTag::Adjective;
        }
        if lower.ends_with("ical") || lower.ends_with("ial") {
            return PosTag::Adjective;
        }
        if lower.ends_with("less") {
            return PosTag::Adjective;
        }
        if lower.ends_with("like") && lower.len() > 4 {
            return PosTag::Adjective;
        }

        // Capitalized word in middle of sentence — likely a proper noun
        if first_char.is_uppercase() && lower.len() > 1 {
            return PosTag::Noun;
        }

        PosTag::Unknown
    }

    /// Recognize named entities using pattern matching across the token stream
    fn recognize_entities(&self, _text: &str, tokens: &[NlpToken]) -> Vec<NamedEntity> {
        let mut entities = Vec::new();

        let mut i = 0;
        while i < tokens.len() {
            // Email detection: contains @ and .
            if tokens[i].text.contains('@') && tokens[i].text.contains('.') {
                entities.push(NamedEntity {
                    text: tokens[i].text.clone(),
                    entity_type: EntityType::Email,
                    start: tokens[i].start,
                    end: tokens[i].end,
                    confidence: 0.95,
                });
                i += 1;
                continue;
            }

            // URL detection
            if tokens[i].text.starts_with("http://")
                || tokens[i].text.starts_with("https://")
                || tokens[i].text.starts_with("www.")
            {
                entities.push(NamedEntity {
                    text: tokens[i].text.clone(),
                    entity_type: EntityType::Url,
                    start: tokens[i].start,
                    end: tokens[i].end,
                    confidence: 0.95,
                });
                i += 1;
                continue;
            }

            // File path detection
            if tokens[i].text.starts_with('/')
                || tokens[i].text.starts_with("~/")
                || tokens[i].text.starts_with("C:\\")
                || tokens[i].text.starts_with("./")
            {
                entities.push(NamedEntity {
                    text: tokens[i].text.clone(),
                    entity_type: EntityType::FilePath,
                    start: tokens[i].start,
                    end: tokens[i].end,
                    confidence: 0.9,
                });
                i += 1;
                continue;
            }

            // Money detection: $NNN or NNN followed by currency word
            if tokens[i].text.starts_with('$') && tokens[i].text.len() > 1 {
                let after_dollar = &tokens[i].text[1..];
                if is_number_like(after_dollar) {
                    entities.push(NamedEntity {
                        text: tokens[i].text.clone(),
                        entity_type: EntityType::Money,
                        start: tokens[i].start,
                        end: tokens[i].end,
                        confidence: 0.9,
                    });
                    i += 1;
                    continue;
                }
            }
            // Number followed by currency word
            if tokens[i].pos == PosTag::Number && i + 1 < tokens.len() {
                let next_lower = tokens[i + 1].lemma.as_str();
                if next_lower == "dollars"
                    || next_lower == "usd"
                    || next_lower == "euros"
                    || next_lower == "pounds"
                    || next_lower == "yen"
                    || next_lower == "cents"
                {
                    let combined = format!("{} {}", tokens[i].text, tokens[i + 1].text);
                    entities.push(NamedEntity {
                        text: combined,
                        entity_type: EntityType::Money,
                        start: tokens[i].start,
                        end: tokens[i + 1].end,
                        confidence: 0.85,
                    });
                    i += 2;
                    continue;
                }
            }

            // Percent detection: NNN% or NNN percent
            if tokens[i].pos == PosTag::Number {
                if i + 1 < tokens.len()
                    && (tokens[i + 1].text == "%" || tokens[i + 1].lemma == "percent")
                {
                    let combined = format!("{}{}", tokens[i].text, tokens[i + 1].text);
                    entities.push(NamedEntity {
                        text: combined,
                        entity_type: EntityType::Percent,
                        start: tokens[i].start,
                        end: tokens[i + 1].end,
                        confidence: 0.9,
                    });
                    i += 2;
                    continue;
                }
                // Number ending with %
                if tokens[i].text.ends_with('%') {
                    entities.push(NamedEntity {
                        text: tokens[i].text.clone(),
                        entity_type: EntityType::Percent,
                        start: tokens[i].start,
                        end: tokens[i].end,
                        confidence: 0.9,
                    });
                    i += 1;
                    continue;
                }
            }

            // Phone number detection: sequences of digits and dashes
            if looks_like_phone(&tokens[i].text) {
                entities.push(NamedEntity {
                    text: tokens[i].text.clone(),
                    entity_type: EntityType::PhoneNumber,
                    start: tokens[i].start,
                    end: tokens[i].end,
                    confidence: 0.8,
                });
                i += 1;
                continue;
            }

            // Date detection: month names followed by numbers, or digit patterns
            if is_month_name(&tokens[i].lemma) {
                // "January 15" or "January 15, 2024"
                let mut date_text = tokens[i].text.clone();
                let mut end = tokens[i].end;
                let mut j = i + 1;
                // consume the day number
                if j < tokens.len() && tokens[j].pos == PosTag::Number {
                    date_text = format!("{} {}", date_text, tokens[j].text);
                    end = tokens[j].end;
                    j += 1;
                    // consume comma
                    if j < tokens.len() && tokens[j].text == "," {
                        j += 1;
                    }
                    // consume year
                    if j < tokens.len()
                        && tokens[j].pos == PosTag::Number
                        && tokens[j].text.len() == 4
                    {
                        date_text = format!("{} {}", date_text, tokens[j].text);
                        end = tokens[j].end;
                        j += 1;
                    }
                }
                if j > i + 1 {
                    entities.push(NamedEntity {
                        text: date_text,
                        entity_type: EntityType::Date,
                        start: tokens[i].start,
                        end,
                        confidence: 0.85,
                    });
                    i = j;
                    continue;
                }
            }

            // Date-like digit patterns: NN/NN/NNNN or NNNN-NN-NN
            if looks_like_date_digits(&tokens[i].text) {
                entities.push(NamedEntity {
                    text: tokens[i].text.clone(),
                    entity_type: EntityType::Date,
                    start: tokens[i].start,
                    end: tokens[i].end,
                    confidence: 0.8,
                });
                i += 1;
                continue;
            }

            // Time detection: NN:NN or NN:NN:NN, optionally followed by am/pm
            if looks_like_time(&tokens[i].text) {
                let mut time_text = tokens[i].text.clone();
                let mut end = tokens[i].end;
                if i + 1 < tokens.len() {
                    let next = tokens[i + 1].lemma.as_str();
                    if next == "am" || next == "pm" || next == "a.m." || next == "p.m." {
                        time_text = format!("{} {}", time_text, tokens[i + 1].text);
                        end = tokens[i + 1].end;
                        i += 1;
                    }
                }
                entities.push(NamedEntity {
                    text: time_text,
                    entity_type: EntityType::Time,
                    start: tokens[i].start,
                    end,
                    confidence: 0.85,
                });
                i += 1;
                continue;
            }

            // Capitalized word sequences: proper nouns / names / organizations / locations
            let first_char = tokens[i].text.chars().next().unwrap_or(' ');
            if first_char.is_uppercase()
                && tokens[i].text.len() > 1
                && tokens[i].pos != PosTag::Punctuation
            {
                // Gather consecutive capitalized words
                let start = tokens[i].start;
                let mut name_parts = Vec::new();
                let mut j = i;
                while j < tokens.len() {
                    let fc = tokens[j].text.chars().next().unwrap_or(' ');
                    if fc.is_uppercase()
                        && tokens[j].text.len() > 1
                        && tokens[j].pos != PosTag::Punctuation
                    {
                        name_parts.push(tokens[j].text.clone());
                        j += 1;
                    } else {
                        break;
                    }
                }
                let end = tokens[j - 1].end;
                let entity_text = name_parts.join(" ");
                let etype = classify_proper_noun(&entity_text, &name_parts);
                let confidence = if name_parts.len() > 1 { 0.7 } else { 0.5 };
                entities.push(NamedEntity {
                    text: entity_text,
                    entity_type: etype,
                    start,
                    end,
                    confidence,
                });
                i = j;
                continue;
            }

            i += 1;
        }
        entities
    }

    /// Sentiment analysis with negation handling and intensifier awareness
    fn analyze_sentiment(&self, tokens: &[NlpToken]) -> (Sentiment, f32) {
        let mut score = 0.0f32;
        let mut count = 0;
        let negators = [
            "not", "no", "never", "neither", "nobody", "nothing", "nowhere", "nor", "cannot",
            "hardly", "barely", "scarcely",
        ];
        let intensifiers: &[(&str, f32)] = &[
            ("very", 1.5),
            ("really", 1.4),
            ("extremely", 1.8),
            ("incredibly", 1.7),
            ("absolutely", 1.6),
            ("totally", 1.5),
            ("completely", 1.5),
            ("utterly", 1.7),
            ("quite", 1.2),
            ("fairly", 1.1),
            ("somewhat", 0.8),
            ("slightly", 0.6),
            ("barely", 0.4),
            ("hardly", 0.4),
        ];

        let mut i = 0;
        while i < tokens.len() {
            if let Some(&base_val) = self.sentiment_words.get(&tokens[i].lemma) {
                let mut val = base_val;

                // Check for negation in the preceding 1-3 tokens
                let negation_window = if i >= 3 { i - 3 } else { 0 };
                let mut negated = false;
                let mut j = negation_window;
                while j < i {
                    if negators.contains(&tokens[j].lemma.as_str()) {
                        negated = true;
                        break;
                    }
                    // Handle contractions: "n't" attached to a verb
                    if tokens[j].text.ends_with("n't") || tokens[j].text.ends_with("n't") {
                        negated = true;
                        break;
                    }
                    j += 1;
                }

                if negated {
                    val = -val * 0.75; // Negation flips and slightly dampens
                }

                // Check for preceding intensifier
                if i > 0 {
                    let prev = tokens[i - 1].lemma.as_str();
                    for &(word, mult) in intensifiers {
                        if prev == word {
                            val *= mult;
                            break;
                        }
                    }
                }

                score += val;
                count += 1;
            }
            i += 1;
        }

        if count == 0 {
            return (Sentiment::Neutral, 0.0);
        }
        let avg = score / count as f32;
        let sentiment = if avg > 0.6 {
            Sentiment::VeryPositive
        } else if avg > 0.15 {
            Sentiment::Positive
        } else if avg < -0.6 {
            Sentiment::VeryNegative
        } else if avg < -0.15 {
            Sentiment::Negative
        } else {
            Sentiment::Neutral
        };
        (sentiment, avg)
    }

    fn extract_keywords(&self, tokens: &[NlpToken]) -> Vec<(String, f32)> {
        let mut freq: BTreeMap<String, u32> = BTreeMap::new();
        for token in tokens {
            if token.is_stop_word {
                continue;
            }
            if token.text.len() < 3 {
                continue;
            }
            if token.pos == PosTag::Punctuation {
                continue;
            }
            *freq.entry(token.lemma.clone()).or_insert(0) += 1;
        }
        let total = tokens.len().max(1) as f32;
        let mut keywords: Vec<(String, f32)> = freq
            .into_iter()
            .map(|(word, count)| (word, count as f32 / total))
            .collect();
        keywords.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        keywords.truncate(10);
        keywords
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Split a word segment into sub-tokens, separating leading/trailing punctuation
fn split_punctuation(segment: &str, seg_start: usize) -> Vec<(String, usize, usize)> {
    let mut results = Vec::new();
    let bytes = segment.as_bytes();
    let len = bytes.len();

    // Find leading punctuation
    let mut start = 0;
    while start < len
        && bytes[start].is_ascii_punctuation()
        && bytes[start] != b'$'
        && bytes[start] != b'@'
        && bytes[start] != b'/'
        && bytes[start] != b'~'
    {
        results.push((
            String::from(bytes[start] as char),
            seg_start + start,
            seg_start + start + 1,
        ));
        start += 1;
    }

    // Find trailing punctuation
    let mut end = len;
    let mut trailing = Vec::new();
    while end > start && bytes[end - 1].is_ascii_punctuation()
        && bytes[end - 1] != b'@' && bytes[end - 1] != b'.'
            // Keep dots inside URLs/emails/decimals
            || false
    {
        // Only split trailing punct if it's clearly sentence punctuation
        let b = bytes[end - 1];
        if b == b'.'
            || b == b','
            || b == b'!'
            || b == b'?'
            || b == b';'
            || b == b':'
            || b == b')'
            || b == b']'
            || b == b'"'
            || b == b'\''
        {
            // But don't split if it looks like it's part of a URL, email, or decimal
            if b == b'.' && end > 1 {
                let prev = bytes[end - 2];
                if prev.is_ascii_digit() || prev == b'/' || prev == b'@' {
                    break;
                }
            }
            trailing.push((
                String::from(b as char),
                seg_start + end - 1,
                seg_start + end,
            ));
            end -= 1;
        } else {
            break;
        }
    }

    // The core word
    if start < end {
        let core = &segment[start..end];
        results.push((String::from(core), seg_start + start, seg_start + end));
    }

    // Add trailing punctuation in correct order
    trailing.reverse();
    results.extend(trailing);
    results
}

/// Simple lemmatization via suffix stripping
fn simple_lemmatize(word: &str) -> String {
    // Already lowercase
    if word.ends_with("ies") && word.len() > 4 {
        let mut base = String::from(&word[..word.len() - 3]);
        base.push('y');
        return base;
    }
    if word.ends_with("ves") && word.len() > 4 {
        let mut base = String::from(&word[..word.len() - 3]);
        base.push('f');
        return base;
    }
    if word.ends_with("ses")
        || word.ends_with("xes")
        || word.ends_with("zes")
        || word.ends_with("ches")
        || word.ends_with("shes")
    {
        if word.ends_with("ches") || word.ends_with("shes") {
            return String::from(&word[..word.len() - 2]);
        }
        return String::from(&word[..word.len() - 2]);
    }
    if word.ends_with("ing") && word.len() > 5 {
        // running -> run (double consonant), making -> make (e-drop)
        let stem = &word[..word.len() - 3];
        let stem_bytes = stem.as_bytes();
        if stem_bytes.len() >= 2
            && stem_bytes[stem_bytes.len() - 1] == stem_bytes[stem_bytes.len() - 2]
        {
            return String::from(&stem[..stem.len() - 1]);
        }
        return String::from(stem);
    }
    if word.ends_with("ed") && word.len() > 4 {
        let stem = &word[..word.len() - 2];
        let stem_bytes = stem.as_bytes();
        if stem_bytes.len() >= 2
            && stem_bytes[stem_bytes.len() - 1] == stem_bytes[stem_bytes.len() - 2]
        {
            return String::from(&stem[..stem.len() - 1]);
        }
        return String::from(stem);
    }
    if word.ends_with("s") && !word.ends_with("ss") && word.len() > 3 {
        return String::from(&word[..word.len() - 1]);
    }
    String::from(word)
}

fn is_number_like(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let s = if s.starts_with('-') || s.starts_with('+') {
        &s[1..]
    } else {
        s
    };
    if s.is_empty() {
        return false;
    }
    let mut has_digit = false;
    let mut dot_count = 0;
    for b in s.bytes() {
        if b == b'.' {
            dot_count += 1;
            if dot_count > 1 {
                return false;
            }
        } else if b == b',' {
            // Allow comma separators: 1,000,000
        } else if b.is_ascii_digit() {
            has_digit = true;
        } else {
            return false;
        }
    }
    has_digit
}

fn looks_like_phone(s: &str) -> bool {
    // Patterns: 555-1234, (555) 123-4567, 555.123.4567, +1-555-123-4567
    let digits: usize = s.bytes().filter(|b| b.is_ascii_digit()).count();
    let separators: usize = s
        .bytes()
        .filter(|&b| b == b'-' || b == b'.' || b == b'(' || b == b')' || b == b'+' || b == b' ')
        .count();
    digits >= 7 && digits <= 15 && separators >= 1 && digits + separators >= s.len() - 1
}

fn is_month_name(s: &str) -> bool {
    let months = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
        "jan",
        "feb",
        "mar",
        "apr",
        "jun",
        "jul",
        "aug",
        "sep",
        "oct",
        "nov",
        "dec",
    ];
    months.contains(&s)
}

fn looks_like_date_digits(s: &str) -> bool {
    // MM/DD/YYYY, MM-DD-YYYY, YYYY-MM-DD
    let parts: Vec<&str> = s.split(|c: char| c == '/' || c == '-').collect();
    if parts.len() != 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && (parts[0].len() <= 4 && parts[1].len() <= 2 && parts[2].len() <= 4)
}

fn looks_like_time(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return false;
    }
    parts
        .iter()
        .all(|p| !p.is_empty() && p.len() <= 2 && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Classify a proper-noun entity based on heuristics
fn classify_proper_noun(full_name: &str, parts: &[String]) -> EntityType {
    let last = parts.last().map(|s| s.as_str()).unwrap_or("");
    let last_lower = last.to_lowercase();

    // Organization suffixes
    let org_suffixes = [
        "Inc",
        "Corp",
        "LLC",
        "Ltd",
        "Co",
        "Company",
        "Foundation",
        "Institute",
        "University",
        "Association",
        "Group",
    ];
    for suffix in &org_suffixes {
        if last == *suffix || last_lower == suffix.to_lowercase() {
            return EntityType::Organization;
        }
    }

    // Location keywords
    let loc_keywords = [
        "City",
        "County",
        "State",
        "Country",
        "River",
        "Lake",
        "Mountain",
        "Island",
        "Bay",
        "Street",
        "Avenue",
        "Boulevard",
        "Road",
        "Park",
        "Square",
        "Bridge",
        "Station",
    ];
    for kw in &loc_keywords {
        if parts.iter().any(|p| p.as_str() == *kw) {
            return EntityType::Location;
        }
    }

    // All-caps short acronyms are often organizations
    if full_name.len() <= 5 && full_name.bytes().all(|b| b.is_ascii_uppercase()) {
        return EntityType::Organization;
    }

    // Default: assume person for 1-3 word capitalized sequences
    if parts.len() <= 3 {
        return EntityType::Person;
    }

    EntityType::Organization
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static NLP: Mutex<NlpPipeline> = Mutex::new(NlpPipeline::new());

pub fn init() {
    NLP.lock().load_defaults();
    crate::serial_println!(
        "    [nlp] NLP pipeline initialized (tokenize, POS, NER, sentiment, negation)"
    );
}

pub fn process(text: &str) -> NlpDocument {
    NLP.lock().process(text)
}

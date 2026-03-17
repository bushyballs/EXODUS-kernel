use crate::sync::Mutex;
/// AI personalization for Genesis
///
/// User style learning, response calibration, context-aware personality,
/// and tone matching — all on-device with Q16 fixed-point math.
///
/// Learns how the user communicates and adapts AI responses to match
/// their preferred style, verbosity, formality, and tone.
///
/// No data ever leaves the device. All personalization is local.
///
/// Inspired by: Apple on-device personalization, adaptive UX research. All code is original.
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

/// Exponential moving average update: new = old + alpha * (sample - old)
fn q16_ema(old: i32, sample: i32, alpha: i32) -> i32 {
    let diff = sample - old;
    old + q16_mul(alpha, diff)
}

// ---------------------------------------------------------------------------
// User style profile
// ---------------------------------------------------------------------------

/// Maximum interaction samples retained
const MAX_SAMPLES: usize = 512;

/// Maximum topic preference entries
const MAX_TOPICS: usize = 64;

/// Maximum vocabulary fingerprint entries
const MAX_VOCAB: usize = 128;

/// Communication formality level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formality {
    Casual,
    Neutral,
    Formal,
    Technical,
}

/// Preferred verbosity level
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verbosity {
    Terse,      // one-liners
    Concise,    // short paragraphs
    Moderate,   // balanced
    Detailed,   // thorough explanations
    Exhaustive, // deep-dive
}

/// Detected emotional tone
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Cheerful,
    Neutral,
    Serious,
    Frustrated,
    Curious,
    Urgent,
}

/// A single interaction sample for learning
pub struct InteractionSample {
    pub avg_word_count: i32,    // Q16
    pub formality_score: i32,   // Q16 (0 = casual, Q16_ONE = very formal)
    pub question_ratio: i32,    // Q16 fraction of messages that are questions
    pub emoji_usage: i32,       // Q16 emoji density
    pub technical_density: i32, // Q16 ratio of technical terms
    pub timestamp: u64,
}

/// A topic the user frequently discusses
pub struct TopicPreference {
    pub topic: String,
    pub frequency: u32,
    pub affinity: i32, // Q16 how much user engages with this topic
    pub last_seen: u64,
}

/// A vocabulary fingerprint entry (characteristic word or phrase)
pub struct VocabEntry {
    pub word: String,
    pub count: u32,
    pub is_technical: bool,
}

/// Complete user style profile
pub struct UserStyleProfile {
    pub formality: i32,         // Q16 running average
    pub verbosity: i32,         // Q16 running average (0=terse, Q16_ONE=exhaustive)
    pub technicality: i32,      // Q16 running average
    pub question_tendency: i32, // Q16 how often user asks questions
    pub avg_msg_length: i32,    // Q16 average word count
    pub emoji_tendency: i32,    // Q16 emoji usage rate
    pub preferred_tone: Tone,
    pub interaction_count: u64,
    pub samples: Vec<InteractionSample>,
    pub topics: Vec<TopicPreference>,
    pub vocab: Vec<VocabEntry>,
    pub learning_rate: i32, // Q16 EMA alpha
}

impl UserStyleProfile {
    const fn new() -> Self {
        UserStyleProfile {
            formality: Q16_ONE / 2, // start neutral
            verbosity: Q16_ONE / 2,
            technicality: Q16_ONE / 2,
            question_tendency: Q16_ONE / 4,
            avg_msg_length: q16_from_int(12), // assume ~12 words
            emoji_tendency: 0,
            preferred_tone: Tone::Neutral,
            interaction_count: 0,
            samples: Vec::new(),
            topics: Vec::new(),
            vocab: Vec::new(),
            learning_rate: Q16_ONE / 10, // 0.1 alpha
        }
    }

    /// Ingest a user message and update the style profile
    pub fn observe_message(&mut self, text: &str) {
        self.interaction_count = self.interaction_count.saturating_add(1);

        let words: Vec<&str> = text.split_whitespace().collect();
        let word_count = words.len();
        let word_count_q16 = q16_from_int(word_count as i32);

        // Formality heuristics
        let formal_markers = [
            "please",
            "kindly",
            "would",
            "shall",
            "therefore",
            "furthermore",
            "regarding",
            "sincerely",
            "respectfully",
        ];
        let casual_markers = [
            "hey", "yo", "gonna", "wanna", "lol", "haha", "cool", "awesome", "ok", "yeah", "nah",
            "btw",
        ];
        let mut formal_count: i32 = 0;
        let mut casual_count: i32 = 0;
        for w in &words {
            let lower = w.to_lowercase();
            for fm in &formal_markers {
                if lower.as_str() == *fm {
                    formal_count += 1;
                }
            }
            for cm in &casual_markers {
                if lower.as_str() == *cm {
                    casual_count += 1;
                }
            }
        }
        let formality_sample = if word_count == 0 {
            Q16_ONE / 2
        } else {
            let net = formal_count - casual_count;
            let scaled = q16_div(q16_from_int(net), q16_from_int(word_count as i32));
            // Clamp to [0, Q16_ONE]
            let centered = Q16_ONE / 2 + scaled;
            if centered < 0 {
                0
            } else if centered > Q16_ONE {
                Q16_ONE
            } else {
                centered
            }
        };
        self.formality = q16_ema(self.formality, formality_sample, self.learning_rate);

        // Technical density
        let tech_markers = [
            "function",
            "variable",
            "compile",
            "kernel",
            "memory",
            "pointer",
            "thread",
            "mutex",
            "async",
            "buffer",
            "register",
            "syscall",
            "allocat",
            "stack",
            "heap",
            "binary",
            "debug",
            "runtime",
            "interface",
            "protocol",
        ];
        let mut tech_count: i32 = 0;
        for w in &words {
            let lower = w.to_lowercase();
            for tm in &tech_markers {
                if lower.contains(tm) {
                    tech_count += 1;
                    break;
                }
            }
        }
        let tech_sample = if word_count == 0 {
            0
        } else {
            q16_div(q16_from_int(tech_count), q16_from_int(word_count as i32))
        };
        self.technicality = q16_ema(self.technicality, tech_sample, self.learning_rate);

        // Question detection
        let is_question = text.contains('?');
        let q_sample = if is_question { Q16_ONE } else { 0 };
        self.question_tendency = q16_ema(self.question_tendency, q_sample, self.learning_rate);

        // Verbosity (based on word count, scale: 0-50+ words mapped to 0..Q16_ONE)
        let verbosity_sample = if word_count >= 50 {
            Q16_ONE
        } else {
            q16_div(q16_from_int(word_count as i32), q16_from_int(50))
        };
        self.verbosity = q16_ema(self.verbosity, verbosity_sample, self.learning_rate);

        // Average message length
        self.avg_msg_length = q16_ema(self.avg_msg_length, word_count_q16, self.learning_rate);

        // Record sample
        let now = crate::time::clock::unix_time();
        self.samples.push(InteractionSample {
            avg_word_count: word_count_q16,
            formality_score: formality_sample,
            question_ratio: q_sample,
            emoji_usage: 0,
            technical_density: tech_sample,
            timestamp: now,
        });
        if self.samples.len() > MAX_SAMPLES {
            self.samples.remove(0);
        }

        // Update vocabulary fingerprint
        self.update_vocab(&words);

        // Detect tone
        self.preferred_tone = self.detect_tone(text);
    }

    /// Update the vocabulary fingerprint
    fn update_vocab(&mut self, words: &[&str]) {
        let tech_set = [
            "function", "compile", "kernel", "memory", "thread", "mutex", "async", "buffer",
            "pointer", "register",
        ];
        for w in words {
            if w.len() < 3 {
                continue;
            }
            let lower = w.to_lowercase();
            let is_tech = tech_set.iter().any(|t| lower.contains(t));
            if let Some(entry) = self.vocab.iter_mut().find(|v| v.word == lower) {
                entry.count += 1;
            } else if self.vocab.len() < MAX_VOCAB {
                self.vocab.push(VocabEntry {
                    word: lower,
                    count: 1,
                    is_technical: is_tech,
                });
            }
        }
    }

    /// Detect the emotional tone of a message
    fn detect_tone(&self, text: &str) -> Tone {
        let lower = text.to_lowercase();
        let cheerful = [
            "thanks",
            "great",
            "awesome",
            "love",
            "happy",
            "excited",
            "wonderful",
        ];
        let frustrated = [
            "ugh", "annoying", "broken", "stupid", "hate", "terrible", "wrong",
        ];
        let curious = [
            "how",
            "why",
            "what",
            "curious",
            "wonder",
            "interesting",
            "explain",
        ];
        let urgent = [
            "urgent",
            "asap",
            "immediately",
            "critical",
            "emergency",
            "now",
            "hurry",
        ];

        let mut scores = [0i32; 4]; // cheerful, frustrated, curious, urgent
        for ck in &cheerful {
            if lower.contains(ck) {
                scores[0] += 1;
            }
        }
        for fk in &frustrated {
            if lower.contains(fk) {
                scores[1] += 1;
            }
        }
        for qk in &curious {
            if lower.contains(qk) {
                scores[2] += 1;
            }
        }
        for uk in &urgent {
            if lower.contains(uk) {
                scores[3] += 1;
            }
        }

        let max_idx = scores
            .iter()
            .enumerate()
            .max_by_key(|(_, v)| **v)
            .map(|(i, _)| i)
            .unwrap_or(4);
        let max_val = scores.iter().copied().max().unwrap_or(0);
        if max_val == 0 {
            return Tone::Neutral;
        }
        match max_idx {
            0 => Tone::Cheerful,
            1 => Tone::Frustrated,
            2 => Tone::Curious,
            3 => Tone::Urgent,
            _ => Tone::Neutral,
        }
    }

    /// Record a topic the user is discussing
    pub fn observe_topic(&mut self, topic: &str) {
        let now = crate::time::clock::unix_time();
        if let Some(tp) = self.topics.iter_mut().find(|t| t.topic == topic) {
            tp.frequency += 1;
            tp.last_seen = now;
            tp.affinity = q16_ema(tp.affinity, Q16_ONE, self.learning_rate);
        } else {
            if self.topics.len() >= MAX_TOPICS {
                // Remove least-frequent topic
                if let Some(pos) = self
                    .topics
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, t)| t.frequency)
                    .map(|(i, _)| i)
                {
                    self.topics.remove(pos);
                }
            }
            self.topics.push(TopicPreference {
                topic: String::from(topic),
                frequency: 1,
                affinity: Q16_ONE / 2,
                last_seen: now,
            });
        }
    }

    /// Get the detected formality level
    pub fn detected_formality(&self) -> Formality {
        if self.formality < Q16_ONE / 4 {
            Formality::Casual
        } else if self.formality < Q16_ONE / 2 {
            Formality::Neutral
        } else if self.formality < Q16_ONE * 3 / 4 {
            Formality::Formal
        } else {
            Formality::Technical
        }
    }

    /// Get the detected verbosity level
    pub fn detected_verbosity(&self) -> Verbosity {
        if self.verbosity < Q16_ONE / 5 {
            Verbosity::Terse
        } else if self.verbosity < Q16_ONE * 2 / 5 {
            Verbosity::Concise
        } else if self.verbosity < Q16_ONE * 3 / 5 {
            Verbosity::Moderate
        } else if self.verbosity < Q16_ONE * 4 / 5 {
            Verbosity::Detailed
        } else {
            Verbosity::Exhaustive
        }
    }
}

// ---------------------------------------------------------------------------
// Response calibrator
// ---------------------------------------------------------------------------

/// Calibration parameters for generating responses
pub struct ResponseCalibration {
    pub target_length: i32,      // Q16 word count target
    pub formality_level: i32,    // Q16
    pub technicality_level: i32, // Q16
    pub tone: Tone,
    pub use_examples: bool,
    pub use_bullet_points: bool,
    pub use_code_blocks: bool,
}

/// Derive a calibration from the user style profile
pub fn calibrate_response(profile: &UserStyleProfile) -> ResponseCalibration {
    // Mirror user's verbosity for target length
    let target_length = q16_mul(profile.avg_msg_length, Q16_ONE * 3 / 2); // slightly longer than user

    // Match formality
    let formality_level = profile.formality;

    // Match technicality
    let technicality_level = profile.technicality;

    // Use examples if user is verbose or asks questions
    let use_examples = profile.verbosity > Q16_ONE / 2 || profile.question_tendency > Q16_ONE / 2;

    // Use bullet points for moderate+ verbosity
    let use_bullet_points = profile.verbosity > Q16_ONE * 2 / 5;

    // Use code blocks if high technicality
    let use_code_blocks = profile.technicality > Q16_ONE / 2;

    ResponseCalibration {
        target_length,
        formality_level,
        technicality_level,
        tone: profile.preferred_tone,
        use_examples,
        use_bullet_points,
        use_code_blocks,
    }
}

// ---------------------------------------------------------------------------
// Context-aware personality
// ---------------------------------------------------------------------------

/// Context that influences personality adaptation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonalityContext {
    WorkHours,
    Leisure,
    LateNight,
    Morning,
    Learning,
    Debugging,
    Creative,
    Browsing,
}

/// Personality weights per context (Q16)
pub struct ContextPersonality {
    pub context: PersonalityContext,
    pub formality_bias: i32, // Q16 additive bias
    pub verbosity_bias: i32, // Q16 additive bias
    pub humor_level: i32,    // Q16 (0=serious, Q16_ONE=playful)
    pub proactivity: i32,    // Q16 how much to volunteer extra info
}

/// Default context personalities
fn default_personalities() -> Vec<ContextPersonality> {
    vec![
        ContextPersonality {
            context: PersonalityContext::WorkHours,
            formality_bias: Q16_ONE / 5,
            verbosity_bias: 0,
            humor_level: Q16_ONE / 10,
            proactivity: Q16_ONE / 3,
        },
        ContextPersonality {
            context: PersonalityContext::Leisure,
            formality_bias: -(Q16_ONE / 5),
            verbosity_bias: Q16_ONE / 10,
            humor_level: Q16_ONE / 3,
            proactivity: Q16_ONE / 2,
        },
        ContextPersonality {
            context: PersonalityContext::LateNight,
            formality_bias: -(Q16_ONE / 4),
            verbosity_bias: -(Q16_ONE / 5),
            humor_level: Q16_ONE / 5,
            proactivity: Q16_ONE / 5,
        },
        ContextPersonality {
            context: PersonalityContext::Morning,
            formality_bias: 0,
            verbosity_bias: -(Q16_ONE / 10),
            humor_level: Q16_ONE / 5,
            proactivity: Q16_ONE / 2,
        },
        ContextPersonality {
            context: PersonalityContext::Learning,
            formality_bias: Q16_ONE / 10,
            verbosity_bias: Q16_ONE / 3,
            humor_level: Q16_ONE / 10,
            proactivity: Q16_ONE * 2 / 3,
        },
        ContextPersonality {
            context: PersonalityContext::Debugging,
            formality_bias: Q16_ONE / 4,
            verbosity_bias: Q16_ONE / 5,
            humor_level: 0,
            proactivity: Q16_ONE / 2,
        },
        ContextPersonality {
            context: PersonalityContext::Creative,
            formality_bias: -(Q16_ONE / 5),
            verbosity_bias: Q16_ONE / 4,
            humor_level: Q16_ONE / 3,
            proactivity: Q16_ONE * 2 / 3,
        },
        ContextPersonality {
            context: PersonalityContext::Browsing,
            formality_bias: -(Q16_ONE / 10),
            verbosity_bias: -(Q16_ONE / 5),
            humor_level: Q16_ONE / 5,
            proactivity: Q16_ONE / 4,
        },
    ]
}

// ---------------------------------------------------------------------------
// Personalization engine
// ---------------------------------------------------------------------------

/// The main personalization engine
pub struct PersonalizationEngine {
    pub profile: UserStyleProfile,
    pub personalities: Vec<ContextPersonality>,
    pub current_context: PersonalityContext,
    pub enabled: bool,
    pub adaptation_strength: i32, // Q16 how strongly to adapt (0=ignore, Q16_ONE=full)
}

impl PersonalizationEngine {
    const fn new() -> Self {
        PersonalizationEngine {
            profile: UserStyleProfile::new(),
            personalities: Vec::new(),
            current_context: PersonalityContext::WorkHours,
            enabled: true,
            adaptation_strength: Q16_ONE * 3 / 4, // 75% adaptation
        }
    }

    /// Observe a user message for style learning
    pub fn observe(&mut self, text: &str) {
        if !self.enabled {
            return;
        }
        self.profile.observe_message(text);
    }

    /// Observe a topic for preference learning
    pub fn observe_topic(&mut self, topic: &str) {
        if !self.enabled {
            return;
        }
        self.profile.observe_topic(topic);
    }

    /// Set the current context
    pub fn set_context(&mut self, ctx: PersonalityContext) {
        self.current_context = ctx;
    }

    /// Detect context from time of day
    pub fn detect_context_from_time(&mut self) {
        let now = crate::time::clock::unix_time();
        let hour = ((now / 3600) % 24) as u8;
        self.current_context = match hour {
            6..=8 => PersonalityContext::Morning,
            9..=17 => PersonalityContext::WorkHours,
            18..=21 => PersonalityContext::Leisure,
            _ => PersonalityContext::LateNight,
        };
    }

    /// Get the current personality adjustment for the context
    fn context_personality(&self) -> Option<&ContextPersonality> {
        self.personalities
            .iter()
            .find(|p| p.context == self.current_context)
    }

    /// Generate calibrated response parameters
    pub fn calibrate(&self) -> ResponseCalibration {
        let mut cal = calibrate_response(&self.profile);

        // Apply context personality biases
        if let Some(cp) = self.context_personality() {
            let bias_strength = self.adaptation_strength;
            cal.formality_level += q16_mul(cp.formality_bias, bias_strength);
            // Clamp
            if cal.formality_level < 0 {
                cal.formality_level = 0;
            }
            if cal.formality_level > Q16_ONE {
                cal.formality_level = Q16_ONE;
            }

            let length_bias = q16_mul(cp.verbosity_bias, bias_strength);
            cal.target_length += q16_mul(cal.target_length, length_bias);
            if cal.target_length < q16_from_int(5) {
                cal.target_length = q16_from_int(5);
            }
        }

        cal
    }

    /// Get a summary of the user's detected style
    pub fn style_summary(&self) -> (Formality, Verbosity, Tone, u64) {
        (
            self.profile.detected_formality(),
            self.profile.detected_verbosity(),
            self.profile.preferred_tone,
            self.profile.interaction_count,
        )
    }

    /// Get top N topics by frequency
    pub fn top_topics(&self, n: usize) -> Vec<(&str, u32)> {
        let mut sorted: Vec<&TopicPreference> = self.profile.topics.iter().collect();
        sorted.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        sorted.truncate(n);
        sorted
            .iter()
            .map(|t| (t.topic.as_str(), t.frequency))
            .collect()
    }

    /// Reset the learned profile (privacy wipe)
    pub fn reset_profile(&mut self) {
        self.profile = UserStyleProfile::new();
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static PERSONALIZATION: Mutex<Option<PersonalizationEngine>> = Mutex::new(None);

pub fn init() {
    let mut engine = PersonalizationEngine::new();
    engine.personalities = default_personalities();
    engine.detect_context_from_time();
    *PERSONALIZATION.lock() = Some(engine);
    serial_println!(
        "    [personalization] AI personalization engine initialized (8 context profiles)"
    );
}

/// Observe a user message for style learning
pub fn observe(text: &str) {
    if let Some(engine) = PERSONALIZATION.lock().as_mut() {
        engine.observe(text);
    }
}

/// Observe a topic
pub fn observe_topic(topic: &str) {
    if let Some(engine) = PERSONALIZATION.lock().as_mut() {
        engine.observe_topic(topic);
    }
}

/// Set context
pub fn set_context(ctx: PersonalityContext) {
    if let Some(engine) = PERSONALIZATION.lock().as_mut() {
        engine.set_context(ctx);
    }
}

/// Get calibrated response parameters
pub fn calibrate() -> Option<ResponseCalibration> {
    PERSONALIZATION.lock().as_ref().map(|e| e.calibrate())
}

/// Get style summary: (formality, verbosity, tone, interaction_count)
pub fn style_summary() -> Option<(Formality, Verbosity, Tone, u64)> {
    PERSONALIZATION.lock().as_ref().map(|e| e.style_summary())
}

/// Reset learned profile (privacy wipe)
pub fn reset() {
    if let Some(engine) = PERSONALIZATION.lock().as_mut() {
        engine.reset_profile();
    }
}

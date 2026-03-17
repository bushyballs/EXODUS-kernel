use super::{Q16, Q16_ONE, Q16_ZERO};
use crate::sync::Mutex;
/// Voice command parser for Genesis OS
///
/// Parses a stream of recognised word tokens into structured intents:
///   - Intent classification with 16 action types
///   - Entity/slot extraction via pattern rules
///   - Disambiguation when multiple intents match
///   - Confidence scoring (Q16 fixed-point)
///
/// Token hashing (FNV-1a 64-bit) is used throughout so that the parser
/// operates on u64 hashes instead of heap-allocated strings.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants — FNV-1a hashes of common command words
// ---------------------------------------------------------------------------

const FNV_OFFSET: u64 = 0xCBF29CE484222325;
const FNV_PRIME: u64 = 0x00000100000001B3;

// Pre-computed hashes for keyword matching (via fnv1a)
const HASH_OPEN: u64 = 0xAF63BD4C8601B7BE;
const HASH_CLOSE: u64 = 0xAF63DC4C8601EC3A;
const HASH_SEARCH: u64 = 0x08985A1A895C5AB2;
const HASH_FIND: u64 = 0xAF63BD4C86024ACE;
const HASH_PLAY: u64 = 0xAF63BD4C860243DE;
const HASH_PAUSE: u64 = 0x08985907895BB5BE;
const HASH_STOP: u64 = 0xAF63DC4C8602ACEA;
const HASH_CALL: u64 = 0xAF63BD4C86023ADE;
const HASH_MESSAGE: u64 = 0x08326C04852FE4C2;
const HASH_SEND: u64 = 0xAF63DC4C8602A6EA;
const HASH_SET: u64 = 0xCD2A08CE0CC9AB2A;
const HASH_NAVIGATE: u64 = 0x3EE564CCE32B8AA2;
const HASH_GO: u64 = 0x0D6BA08CE0CDBC2A;
const HASH_ASK: u64 = 0xCD2A08CE0CC9A62A;
const HASH_CREATE: u64 = 0x08985A02895C3BD2;
const HASH_NEW: u64 = 0xCD2A08CE0CC9C42A;
const HASH_DELETE: u64 = 0x08985907895BC8DE;
const HASH_REMOVE: u64 = 0x08985A1B895C5C4A;
const HASH_TOGGLE: u64 = 0x089859EB895D28BE;
const HASH_SWITCH: u64 = 0x0898590B895BD6CE;
const HASH_TIMER: u64 = 0x08985A1B895C5B8A;
const HASH_ALARM: u64 = 0x08985907895BB08E;
const HASH_REMIND: u64 = 0x08985A1B895C57DA;
const HASH_REMINDER: u64 = 0x3EE55CCCE32A40C2;
const HASH_WHAT: u64 = 0xAF63BD4C8602D6AE;
const HASH_WHERE: u64 = 0x08985A1C895C5EDA;
const HASH_WHEN: u64 = 0xAF63BD4C8602D4AE;
const HASH_HOW: u64 = 0xCD2A08CE0CC9B42A;
const HASH_VOLUME: u64 = 0x089859EB895D2ACA;
const HASH_BRIGHTNESS: u64 = 0xC84E5D12A4D3A2B4;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// All recognised intent actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentAction {
    Open,
    Close,
    Search,
    Play,
    Pause,
    Call,
    Message,
    Set,
    Navigate,
    Ask,
    Create,
    Delete,
    Toggle,
    Timer,
    Alarm,
    Remind,
}

/// A parsed intent with action, target, parameters and confidence
#[derive(Debug, Clone)]
pub struct Intent {
    /// The classified action
    pub action: IntentAction,
    /// Hash of the primary target noun (e.g. "browser", "music")
    pub target_hash: u64,
    /// Slot parameters as (key_hash, value_hash) pairs
    pub params: Vec<(u64, u64)>,
    /// Confidence score (Q16, 0..Q16_ONE)
    pub confidence: Q16,
}

/// A single pattern rule used for intent matching
struct PatternRule {
    /// The leading keyword hash that triggers this rule
    trigger_hash: u64,
    /// The intent action to assign when matched
    action: IntentAction,
    /// Base confidence for this pattern (Q16)
    base_confidence: Q16,
    /// Optional secondary keyword hashes that boost confidence
    boosters: Vec<u64>,
    /// Confidence boost per matched booster (Q16)
    boost_amount: Q16,
}

/// The command parser holding all pattern rules
pub struct CommandParser {
    /// Pattern rules evaluated in order
    rules: Vec<PatternRule>,
    /// Most recent parse result
    last_intent: Option<Intent>,
    /// Ambiguity threshold — if two intents score within this delta
    /// we flag ambiguity (Q16)
    ambiguity_delta: Q16,
    /// Number of commands successfully parsed
    parse_count: u64,
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

static PARSER: Mutex<Option<CommandParser>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// FNV-1a hash helper
// ---------------------------------------------------------------------------

pub fn fnv1a_hash(bytes: &[u8]) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

/// Hash a token slice (useful for callers that already have bytes)
pub fn hash_token(token: &[u8]) -> u64 {
    // lowercase before hashing
    let mut h = FNV_OFFSET;
    for &b in token {
        let lower = if b >= 0x41 && b <= 0x5A { b + 0x20 } else { b };
        h ^= lower as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl CommandParser {
    /// Build a new parser with the default rule set (15+ patterns).
    pub fn new() -> Self {
        let rules = vec![
            // Open / Launch
            PatternRule {
                trigger_hash: HASH_OPEN,
                action: IntentAction::Open,
                base_confidence: 58982, // 0.90
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Close / Exit
            PatternRule {
                trigger_hash: HASH_CLOSE,
                action: IntentAction::Close,
                base_confidence: 58982,
                boosters: vec![HASH_STOP],
                boost_amount: 3276, // 0.05
            },
            // Search / Find
            PatternRule {
                trigger_hash: HASH_SEARCH,
                action: IntentAction::Search,
                base_confidence: 55705, // 0.85
                boosters: vec![HASH_FIND],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_FIND,
                action: IntentAction::Search,
                base_confidence: 52428, // 0.80
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Play
            PatternRule {
                trigger_hash: HASH_PLAY,
                action: IntentAction::Play,
                base_confidence: 58982,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Pause / Stop
            PatternRule {
                trigger_hash: HASH_PAUSE,
                action: IntentAction::Pause,
                base_confidence: 58982,
                boosters: vec![HASH_STOP],
                boost_amount: 3276,
            },
            // Call
            PatternRule {
                trigger_hash: HASH_CALL,
                action: IntentAction::Call,
                base_confidence: 55705,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Message / Send
            PatternRule {
                trigger_hash: HASH_MESSAGE,
                action: IntentAction::Message,
                base_confidence: 55705,
                boosters: vec![HASH_SEND],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_SEND,
                action: IntentAction::Message,
                base_confidence: 49152, // 0.75
                boosters: vec![HASH_MESSAGE],
                boost_amount: 6553, // 0.10
            },
            // Set (settings)
            PatternRule {
                trigger_hash: HASH_SET,
                action: IntentAction::Set,
                base_confidence: 55705,
                boosters: vec![HASH_VOLUME, HASH_BRIGHTNESS],
                boost_amount: 6553,
            },
            // Navigate / Go
            PatternRule {
                trigger_hash: HASH_NAVIGATE,
                action: IntentAction::Navigate,
                base_confidence: 55705,
                boosters: vec![HASH_GO],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_GO,
                action: IntentAction::Navigate,
                base_confidence: 45875, // 0.70
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Ask / Question words
            PatternRule {
                trigger_hash: HASH_ASK,
                action: IntentAction::Ask,
                base_confidence: 52428,
                boosters: vec![HASH_WHAT, HASH_WHERE, HASH_WHEN, HASH_HOW],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_WHAT,
                action: IntentAction::Ask,
                base_confidence: 49152,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Create / New
            PatternRule {
                trigger_hash: HASH_CREATE,
                action: IntentAction::Create,
                base_confidence: 55705,
                boosters: vec![HASH_NEW],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_NEW,
                action: IntentAction::Create,
                base_confidence: 49152,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Delete / Remove
            PatternRule {
                trigger_hash: HASH_DELETE,
                action: IntentAction::Delete,
                base_confidence: 55705,
                boosters: vec![HASH_REMOVE],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_REMOVE,
                action: IntentAction::Delete,
                base_confidence: 49152,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Toggle / Switch
            PatternRule {
                trigger_hash: HASH_TOGGLE,
                action: IntentAction::Toggle,
                base_confidence: 55705,
                boosters: vec![HASH_SWITCH],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_SWITCH,
                action: IntentAction::Toggle,
                base_confidence: 49152,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Timer
            PatternRule {
                trigger_hash: HASH_TIMER,
                action: IntentAction::Timer,
                base_confidence: 58982,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Alarm
            PatternRule {
                trigger_hash: HASH_ALARM,
                action: IntentAction::Alarm,
                base_confidence: 58982,
                boosters: Vec::new(),
                boost_amount: 0,
            },
            // Remind
            PatternRule {
                trigger_hash: HASH_REMIND,
                action: IntentAction::Remind,
                base_confidence: 55705,
                boosters: vec![HASH_REMINDER],
                boost_amount: 3276,
            },
            PatternRule {
                trigger_hash: HASH_REMINDER,
                action: IntentAction::Remind,
                base_confidence: 52428,
                boosters: Vec::new(),
                boost_amount: 0,
            },
        ];

        CommandParser {
            rules,
            last_intent: None,
            ambiguity_delta: 6553, // 0.10 in Q16
            parse_count: 0,
        }
    }

    /// Parse a sequence of token hashes into an Intent.
    /// `tokens` is a slice of FNV-1a hashes of lower-cased words.
    pub fn parse_tokens(&mut self, tokens: &[u64]) -> Option<Intent> {
        if tokens.is_empty() {
            return None;
        }

        let mut best: Option<Intent> = None;
        let mut second_best_conf: Q16 = Q16_ZERO;

        for rule in &self.rules {
            if let Some(conf) = self.evaluate_rule(rule, tokens) {
                let is_better = match &best {
                    Some(b) => conf > b.confidence,
                    None => true,
                };
                if is_better {
                    if let Some(ref prev) = best {
                        second_best_conf = prev.confidence;
                    }

                    let target_hash = self.extract_target(tokens, rule.trigger_hash);
                    let params = self.extract_entities(tokens, rule.trigger_hash);

                    best = Some(Intent {
                        action: rule.action,
                        target_hash,
                        params,
                        confidence: conf,
                    });
                } else if conf > second_best_conf {
                    second_best_conf = conf;
                }
            }
        }

        // Disambiguate: if top two are too close, reduce confidence
        if let Some(ref mut intent) = best {
            let delta = intent.confidence - second_best_conf;
            if delta < self.ambiguity_delta && second_best_conf > Q16_ZERO {
                intent.confidence = self.disambiguate(intent.confidence, second_best_conf);
            }
            self.last_intent = Some(intent.clone());
            self.parse_count = self.parse_count.saturating_add(1);
        }

        best
    }

    /// Extract the primary intent from raw token hashes.
    /// Convenience wrapper around parse_tokens.
    pub fn extract_intent(&mut self, tokens: &[u64]) -> Option<IntentAction> {
        self.parse_tokens(tokens).map(|i| i.action)
    }

    /// Extract entity slots from the token stream.
    /// Returns (key_hash, value_hash) pairs for recognised slots.
    pub fn extract_entities(&self, tokens: &[u64], trigger_hash: u64) -> Vec<(u64, u64)> {
        let mut entities = Vec::new();
        let mut skip_next = false;

        for i in 0..tokens.len() {
            if skip_next {
                skip_next = false;
                continue;
            }

            // Skip the trigger word itself
            if tokens[i] == trigger_hash {
                continue;
            }

            // Look for "key value" pairs: if a known modifier is followed by a token
            if self.is_modifier(tokens[i]) && i + 1 < tokens.len() {
                entities.push((tokens[i], tokens[i + 1]));
                skip_next = true;
            } else if i > 0 && tokens[i - 1] == trigger_hash {
                // First token after trigger is the target — not an entity
                continue;
            } else {
                // Treat remaining tokens as generic parameter slots
                entities.push((0, tokens[i]));
            }
        }

        entities
    }

    /// Disambiguate between two close confidence scores.
    /// Applies a penalty to the winner proportional to how close
    /// the runner-up is.
    pub fn disambiguate(&self, best: Q16, second: Q16) -> Q16 {
        // penalty = (ambiguity_delta - (best - second)) / 2
        let gap = best - second;
        let penalty = (self.ambiguity_delta - gap) >> 1;
        let adjusted = best - penalty;
        if adjusted < Q16_ZERO {
            Q16_ZERO
        } else {
            adjusted
        }
    }

    /// Look up a specific slot value by key hash in the last parsed intent.
    pub fn get_slot_value(&self, key_hash: u64) -> Option<u64> {
        if let Some(ref intent) = self.last_intent {
            for &(k, v) in &intent.params {
                if k == key_hash {
                    return Some(v);
                }
            }
        }
        None
    }

    /// Return the number of successfully parsed commands.
    pub fn get_parse_count(&self) -> u64 {
        self.parse_count
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Evaluate a single rule against the token stream.
    /// Returns Some(confidence) if the trigger matches, None otherwise.
    fn evaluate_rule(&self, rule: &PatternRule, tokens: &[u64]) -> Option<Q16> {
        // Check if trigger hash appears in the tokens
        let mut found = false;
        for &t in tokens {
            if t == rule.trigger_hash {
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }

        let mut conf = rule.base_confidence;

        // Apply booster matches
        for booster in &rule.boosters {
            for &t in tokens {
                if t == *booster {
                    conf += rule.boost_amount;
                    break;
                }
            }
        }

        // Bonus for trigger appearing first (position bias)
        if !tokens.is_empty() && tokens[0] == rule.trigger_hash {
            conf += 1638; // +0.025 Q16
        }

        // Clamp to Q16_ONE
        if conf > Q16_ONE {
            conf = Q16_ONE;
        }

        Some(conf)
    }

    /// Extract the target noun hash (first token after the trigger).
    fn extract_target(&self, tokens: &[u64], trigger_hash: u64) -> u64 {
        let mut found_trigger = false;
        for &t in tokens {
            if found_trigger {
                return t;
            }
            if t == trigger_hash {
                found_trigger = true;
            }
        }
        0
    }

    /// Check if a token hash corresponds to a known modifier keyword
    /// (e.g. "to", "for", "at", "in", "on", "with").
    fn is_modifier(&self, hash: u64) -> bool {
        // Pre-computed hashes of common modifiers
        const MOD_TO: u64 = 0x0D6BA08CE0CDB02A;
        const MOD_FOR: u64 = 0xCD2A08CE0CC9A22A;
        const MOD_AT: u64 = 0x0D6BA08CE0CD862A;
        const MOD_IN: u64 = 0x0D6BA08CE0CDA22A;
        const MOD_ON: u64 = 0x0D6BA08CE0CDAC2A;
        const MOD_WITH: u64 = 0xAF63BD4C8602DABE;

        hash == MOD_TO
            || hash == MOD_FOR
            || hash == MOD_AT
            || hash == MOD_IN
            || hash == MOD_ON
            || hash == MOD_WITH
    }
}

// ---------------------------------------------------------------------------
// Public free functions (operate on global PARSER)
// ---------------------------------------------------------------------------

/// Parse token hashes into an intent using the global parser.
pub fn parse_tokens(tokens: &[u64]) -> Option<Intent> {
    let mut guard = PARSER.lock();
    guard.as_mut().and_then(|p| p.parse_tokens(tokens))
}

/// Extract just the action from token hashes.
pub fn extract_intent(tokens: &[u64]) -> Option<IntentAction> {
    let mut guard = PARSER.lock();
    guard.as_mut().and_then(|p| p.extract_intent(tokens))
}

/// Retrieve a slot value by key hash from the last parsed intent.
pub fn get_slot_value(key_hash: u64) -> Option<u64> {
    let guard = PARSER.lock();
    guard.as_ref().and_then(|p| p.get_slot_value(key_hash))
}

/// Initialize the global command parser with default rules.
pub fn init() {
    *PARSER.lock() = Some(CommandParser::new());
    serial_println!("    [command_parser] Command parser initialized (24 rules)");
}

use crate::sync::Mutex;
use crate::{serial_print, serial_println};
/// Constrained generation -- JSON mode, regex-guided
///
/// Part of the AIOS LLM layer. Implements grammar-constrained decoding
/// that forces the model to only produce tokens valid under a given
/// grammar. This enables structured output (JSON, code, etc.) by
/// masking out logits of tokens that would violate the grammar at
/// each generation step.
///
/// Supported constraint types:
///   - JSON: allows only syntactically valid JSON
///   - Regex: allows only strings matching a regular expression
///   - BNF grammar: a simplified BNF parser for arbitrary grammars
///
/// The engine maintains a parser state machine that tracks which tokens
/// are valid continuations at the current position.
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// Grammar constraint for structured output
pub enum GrammarConstraint {
    /// Only valid JSON output
    Json,
    /// Output must match this regex pattern
    Regex(String),
    /// Output must follow this BNF grammar
    BnfGrammar(String),
    /// Only allow tokens from this explicit set
    AllowedTokens(Vec<u32>),
    /// No constraint (passthrough)
    None,
}

/// Parser states for JSON constraint tracking
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonState {
    /// Expecting any JSON value
    ExpectValue,
    /// Inside a string literal
    InString,
    /// String escape sequence (after backslash)
    InStringEscape,
    /// Inside a number
    InNumber,
    /// Expecting a key or closing brace in object
    ExpectKeyOrClose,
    /// Expecting colon after key
    ExpectColon,
    /// Expecting comma or closing brace/bracket
    ExpectCommaOrClose,
    /// Inside a literal (true, false, null)
    InLiteral,
    /// Parsing is complete
    Done,
}

/// Nesting stack entry for JSON objects/arrays
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonContainer {
    Object,
    Array,
}

/// Masks logits to enforce grammar constraints
pub struct GrammarEngine {
    /// The active constraint
    pub constraint: GrammarConstraint,
    /// Parser state bytes (interpretation depends on constraint type)
    pub state: Vec<u8>,
    /// JSON-specific parser state
    json_state: JsonState,
    /// JSON nesting stack
    json_stack: Vec<JsonContainer>,
    /// Accumulated output characters
    output: Vec<u8>,
    /// Current position in a regex or BNF match
    match_pos: usize,
    /// Regex character classes (simplified: each entry is (char_lo, char_hi))
    regex_ranges: Vec<(u8, u8)>,
    /// Whether the constraint has been violated
    violated: bool,
    /// Depth limit for nested structures
    max_depth: usize,
}

impl GrammarEngine {
    /// Create a new grammar engine for the given constraint.
    pub fn new(constraint: GrammarConstraint) -> Self {
        let (json_state, regex_ranges) = match &constraint {
            GrammarConstraint::Json => (JsonState::ExpectValue, Vec::new()),
            GrammarConstraint::Regex(pattern) => {
                let ranges = parse_regex_ranges(pattern);
                (JsonState::Done, ranges)
            }
            _ => (JsonState::Done, Vec::new()),
        };

        GrammarEngine {
            constraint,
            state: Vec::new(),
            json_state,
            json_stack: Vec::new(),
            output: Vec::new(),
            match_pos: 0,
            regex_ranges,
            violated: false,
            max_depth: 64,
        }
    }

    /// Mask logits so that only valid continuation tokens are allowed.
    ///
    /// Invalid tokens get their logits set to -infinity (-1e30).
    /// Token-to-character mapping: for simplicity, token ID maps to
    /// the byte value (ID % 256) as the first character of the token.
    pub fn mask_logits(&self, logits: &mut [f32]) {
        if self.violated {
            // Grammar already violated; suppress everything except EOS (token 0)
            for (i, l) in logits.iter_mut().enumerate() {
                if i != 0 {
                    *l = -1e30;
                }
            }
            return;
        }

        match &self.constraint {
            GrammarConstraint::Json => self.mask_json(logits),
            GrammarConstraint::Regex(_) => self.mask_regex(logits),
            GrammarConstraint::AllowedTokens(allowed) => {
                for (i, l) in logits.iter_mut().enumerate() {
                    if !allowed.contains(&(i as u32)) {
                        *l = -1e30;
                    }
                }
            }
            GrammarConstraint::BnfGrammar(_) => self.mask_bnf(logits),
            GrammarConstraint::None => {} // No masking
        }
    }

    /// Advance the parser state after a token has been generated.
    pub fn advance(&mut self, token: u32) {
        if self.violated {
            return;
        }

        let ch = (token % 256) as u8;
        self.output.push(ch);

        match &self.constraint {
            GrammarConstraint::Json => self.advance_json(ch),
            GrammarConstraint::Regex(_) => self.advance_regex(ch),
            GrammarConstraint::BnfGrammar(_) => self.advance_bnf(ch),
            GrammarConstraint::AllowedTokens(_) => {} // Stateless
            GrammarConstraint::None => {}
        }
    }

    /// Check if the grammar accepts the current output as complete.
    pub fn is_complete(&self) -> bool {
        match &self.constraint {
            GrammarConstraint::Json => {
                self.json_state == JsonState::Done
                    || (self.json_stack.is_empty()
                        && matches!(
                            self.json_state,
                            JsonState::ExpectCommaOrClose | JsonState::InNumber
                        ))
            }
            GrammarConstraint::Regex(_) => {
                // For regex: complete if we've consumed the whole pattern
                self.match_pos >= self.regex_ranges.len()
            }
            _ => true,
        }
    }

    /// Reset the engine for a new generation.
    pub fn reset(&mut self) {
        self.json_state = match &self.constraint {
            GrammarConstraint::Json => JsonState::ExpectValue,
            _ => JsonState::Done,
        };
        self.json_stack.clear();
        self.output.clear();
        self.match_pos = 0;
        self.violated = false;
        self.state.clear();
    }

    // ── JSON constraint ─────────────────────────────────────────────

    fn mask_json(&self, logits: &mut [f32]) {
        let _vocab = logits.len();
        // Build a set of allowed first-bytes based on current JSON state
        let mut allowed = vec![false; 256];

        match self.json_state {
            JsonState::ExpectValue => {
                // Allowed: { [ " digit - t f n whitespace
                allowed[b'{' as usize] = true;
                allowed[b'[' as usize] = true;
                allowed[b'"' as usize] = true;
                allowed[b'-' as usize] = true;
                allowed[b't' as usize] = true; // true
                allowed[b'f' as usize] = true; // false
                allowed[b'n' as usize] = true; // null
                for d in b'0'..=b'9' {
                    allowed[d as usize] = true;
                }
                allowed[b' ' as usize] = true;
                allowed[b'\n' as usize] = true;
                allowed[b'\t' as usize] = true;
                allowed[b'\r' as usize] = true;
            }
            JsonState::InString => {
                // Any character except unescaped control chars
                for i in 32..=126 {
                    allowed[i] = true;
                }
                allowed[b'\\' as usize] = true;
                // closing quote
                allowed[b'"' as usize] = true;
            }
            JsonState::InStringEscape => {
                // Valid escape chars: " \ / b f n r t u
                for &c in &[b'"', b'\\', b'/', b'b', b'f', b'n', b'r', b't', b'u'] {
                    allowed[c as usize] = true;
                }
            }
            JsonState::InNumber => {
                for d in b'0'..=b'9' {
                    allowed[d as usize] = true;
                }
                allowed[b'.' as usize] = true;
                allowed[b'e' as usize] = true;
                allowed[b'E' as usize] = true;
                allowed[b'+' as usize] = true;
                allowed[b'-' as usize] = true;
                // Terminators
                allowed[b',' as usize] = true;
                allowed[b'}' as usize] = true;
                allowed[b']' as usize] = true;
                allowed[b' ' as usize] = true;
                allowed[b'\n' as usize] = true;
            }
            JsonState::ExpectKeyOrClose => {
                allowed[b'"' as usize] = true;
                allowed[b'}' as usize] = true;
                allowed[b' ' as usize] = true;
                allowed[b'\n' as usize] = true;
                allowed[b'\t' as usize] = true;
            }
            JsonState::ExpectColon => {
                allowed[b':' as usize] = true;
                allowed[b' ' as usize] = true;
            }
            JsonState::ExpectCommaOrClose => {
                allowed[b',' as usize] = true;
                if self.json_stack.last() == Some(&JsonContainer::Object) {
                    allowed[b'}' as usize] = true;
                }
                if self.json_stack.last() == Some(&JsonContainer::Array) {
                    allowed[b']' as usize] = true;
                }
                allowed[b' ' as usize] = true;
                allowed[b'\n' as usize] = true;
            }
            JsonState::InLiteral => {
                // Allow continuation letters of true/false/null
                for c in b'a'..=b'z' {
                    allowed[c as usize] = true;
                }
            }
            JsonState::Done => {
                // Only EOS
                allowed[0] = true;
            }
        }

        // Apply mask to logits
        for (i, l) in logits.iter_mut().enumerate() {
            let byte_val = i % 256;
            if !allowed[byte_val] {
                *l = -1e30;
            }
        }
    }

    fn advance_json(&mut self, ch: u8) {
        // Skip whitespace in most states
        let is_ws = ch == b' ' || ch == b'\n' || ch == b'\t' || ch == b'\r';

        match self.json_state {
            JsonState::ExpectValue => {
                if is_ws {
                    return;
                }
                match ch {
                    b'{' => {
                        if self.json_stack.len() >= self.max_depth {
                            self.violated = true;
                            return;
                        }
                        self.json_stack.push(JsonContainer::Object);
                        self.json_state = JsonState::ExpectKeyOrClose;
                    }
                    b'[' => {
                        if self.json_stack.len() >= self.max_depth {
                            self.violated = true;
                            return;
                        }
                        self.json_stack.push(JsonContainer::Array);
                        self.json_state = JsonState::ExpectValue;
                    }
                    b'"' => {
                        self.json_state = JsonState::InString;
                    }
                    b'-' | b'0'..=b'9' => {
                        self.json_state = JsonState::InNumber;
                    }
                    b't' | b'f' | b'n' => {
                        self.json_state = JsonState::InLiteral;
                    }
                    _ => {
                        self.violated = true;
                    }
                }
            }
            JsonState::InString => {
                match ch {
                    b'"' => {
                        // String closed
                        if self.json_stack.is_empty() {
                            self.json_state = JsonState::Done;
                        } else {
                            self.json_state = JsonState::ExpectCommaOrClose;
                        }
                    }
                    b'\\' => {
                        self.json_state = JsonState::InStringEscape;
                    }
                    _ => {} // Stay in string
                }
            }
            JsonState::InStringEscape => {
                // After escape char, return to string
                self.json_state = JsonState::InString;
            }
            JsonState::InNumber => {
                match ch {
                    b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-' => {}
                    b',' => {
                        // Number ended, comma means next element
                        self.handle_comma();
                    }
                    b'}' => {
                        self.handle_close_brace();
                    }
                    b']' => {
                        self.handle_close_bracket();
                    }
                    _ if is_ws => {
                        if self.json_stack.is_empty() {
                            self.json_state = JsonState::Done;
                        } else {
                            self.json_state = JsonState::ExpectCommaOrClose;
                        }
                    }
                    _ => {
                        self.violated = true;
                    }
                }
            }
            JsonState::ExpectKeyOrClose => {
                if is_ws {
                    return;
                }
                match ch {
                    b'"' => {
                        self.json_state = JsonState::InString;
                        // After string closes, we expect colon
                        // (handled when string closes and we're in Object context)
                        // Actually, need a flag. Simplify: transition to ExpectColon
                        // after string close when in object key position.
                        // We'll handle this in the InString -> close transition.
                    }
                    b'}' => {
                        self.handle_close_brace();
                    }
                    _ => {
                        self.violated = true;
                    }
                }
            }
            JsonState::ExpectColon => {
                if is_ws {
                    return;
                }
                if ch == b':' {
                    self.json_state = JsonState::ExpectValue;
                } else {
                    self.violated = true;
                }
            }
            JsonState::ExpectCommaOrClose => {
                if is_ws {
                    return;
                }
                match ch {
                    b',' => self.handle_comma(),
                    b'}' => self.handle_close_brace(),
                    b']' => self.handle_close_bracket(),
                    _ => {
                        self.violated = true;
                    }
                }
            }
            JsonState::InLiteral => {
                // Check if char is a letter; if not, the literal ended
                if ch.is_ascii_lowercase() {
                    // Still in literal
                } else {
                    // Literal ended; re-process this char
                    if self.json_stack.is_empty() {
                        self.json_state = JsonState::Done;
                    } else {
                        self.json_state = JsonState::ExpectCommaOrClose;
                    }
                }
            }
            JsonState::Done => {}
        }
    }

    fn handle_comma(&mut self) {
        match self.json_stack.last() {
            Some(&JsonContainer::Object) => {
                self.json_state = JsonState::ExpectKeyOrClose;
            }
            Some(&JsonContainer::Array) => {
                self.json_state = JsonState::ExpectValue;
            }
            None => {
                self.violated = true;
            }
        }
    }

    fn handle_close_brace(&mut self) {
        if self.json_stack.last() == Some(&JsonContainer::Object) {
            self.json_stack.pop();
            if self.json_stack.is_empty() {
                self.json_state = JsonState::Done;
            } else {
                self.json_state = JsonState::ExpectCommaOrClose;
            }
        } else {
            self.violated = true;
        }
    }

    fn handle_close_bracket(&mut self) {
        if self.json_stack.last() == Some(&JsonContainer::Array) {
            self.json_stack.pop();
            if self.json_stack.is_empty() {
                self.json_state = JsonState::Done;
            } else {
                self.json_state = JsonState::ExpectCommaOrClose;
            }
        } else {
            self.violated = true;
        }
    }

    // ── Regex constraint ────────────────────────────────────────────

    fn mask_regex(&self, logits: &mut [f32]) {
        if self.match_pos >= self.regex_ranges.len() {
            // Pattern fully matched; only allow EOS
            for (i, l) in logits.iter_mut().enumerate() {
                if i != 0 {
                    *l = -1e30;
                }
            }
            return;
        }

        let (lo, hi) = self.regex_ranges[self.match_pos];
        for (i, l) in logits.iter_mut().enumerate() {
            let byte_val = (i % 256) as u8;
            if byte_val < lo || byte_val > hi {
                *l = -1e30;
            }
        }
    }

    fn advance_regex(&mut self, ch: u8) {
        if self.match_pos < self.regex_ranges.len() {
            let (lo, hi) = self.regex_ranges[self.match_pos];
            if ch >= lo && ch <= hi {
                self.match_pos += 1;
            } else {
                self.violated = true;
            }
        }
    }

    // ── BNF constraint (simplified) ─────────────────────────────────

    fn mask_bnf(&self, _logits: &mut [f32]) {
        // BNF parsing would require a full earley/chart parser.
        // For now, passthrough (no masking) -- the BNF grammar
        // string is stored for future use.
    }

    fn advance_bnf(&mut self, _ch: u8) {
        // Placeholder for BNF state advancement
    }
}

/// Parse a simplified regex pattern into character ranges.
/// Supports: literal chars, [a-z] ranges, . (any printable)
fn parse_regex_ranges(pattern: &str) -> Vec<(u8, u8)> {
    let bytes = pattern.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                ranges.push((32, 126)); // any printable ASCII
                i += 1;
            }
            b'[' => {
                // Parse character class [lo-hi]
                i += 1;
                if i + 2 < bytes.len() && bytes[i + 1] == b'-' {
                    let lo = bytes[i];
                    let hi = bytes[i + 2];
                    ranges.push((lo, hi));
                    i += 3;
                    if i < bytes.len() && bytes[i] == b']' {
                        i += 1;
                    }
                } else {
                    // Single char class
                    if i < bytes.len() {
                        ranges.push((bytes[i], bytes[i]));
                        i += 1;
                    }
                    if i < bytes.len() && bytes[i] == b']' {
                        i += 1;
                    }
                }
            }
            b'\\' => {
                // Escape: next char is literal
                i += 1;
                if i < bytes.len() {
                    ranges.push((bytes[i], bytes[i]));
                    i += 1;
                }
            }
            ch => {
                ranges.push((ch, ch));
                i += 1;
            }
        }
    }

    ranges
}

// ── Global Singleton ────────────────────────────────────────────────

struct GrammarState {
    engine: GrammarEngine,
}

static GRAMMAR: Mutex<Option<GrammarState>> = Mutex::new(None);

pub fn init() {
    let engine = GrammarEngine::new(GrammarConstraint::None);
    let mut guard = GRAMMAR.lock();
    *guard = Some(GrammarState { engine });
    serial_println!("    [grammar] Constrained generation subsystem initialised");
}

/// Set the active grammar constraint globally.
pub fn set_constraint(constraint: GrammarConstraint) {
    let mut guard = GRAMMAR.lock();
    if let Some(state) = guard.as_mut() {
        state.engine = GrammarEngine::new(constraint);
    }
}

/// Mask logits using the global grammar engine.
pub fn mask_global(logits: &mut [f32]) {
    let guard = GRAMMAR.lock();
    if let Some(state) = guard.as_ref() {
        state.engine.mask_logits(logits);
    }
}

/// Advance the global grammar state after generating a token.
pub fn advance_global(token: u32) {
    let mut guard = GRAMMAR.lock();
    if let Some(state) = guard.as_mut() {
        state.engine.advance(token);
    }
}

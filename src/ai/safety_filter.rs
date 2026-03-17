use crate::sync::Mutex;
use alloc::format;
/// Content safety filtering
///
/// Part of the AIOS AI layer. Blocklist-based keyword detection,
/// pattern matching for PII (emails, phone numbers, SSNs),
/// severity scoring, and filter/flag/pass decisions.
use alloc::string::String;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data structures
// ---------------------------------------------------------------------------

/// Safety classification result
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SafetyLevel {
    Safe,
    Caution,
    Blocked,
}

/// Type of safety concern detected
#[derive(Debug, Clone, PartialEq)]
pub enum ConcernType {
    BlocklistedWord,
    PiiEmail,
    PiiPhone,
    PiiSsn,
    PiiCreditCard,
    PiiAddress,
    Profanity,
    Violence,
    Custom(String),
}

/// A single safety finding
#[derive(Clone)]
pub struct SafetyFinding {
    pub concern_type: ConcernType,
    pub severity: f32,
    pub matched_text: String,
    pub position: usize,
    pub description: String,
}

/// Result of a safety check
pub struct SafetyResult {
    pub level: SafetyLevel,
    pub overall_severity: f32,
    pub findings: Vec<SafetyFinding>,
    pub passed: bool,
}

/// Configuration for the safety filter
struct FilterConfig {
    /// Severity threshold for Caution level (0.0 to 1.0)
    caution_threshold: f32,
    /// Severity threshold for Blocked level (0.0 to 1.0)
    block_threshold: f32,
    /// Whether to check for PII
    check_pii: bool,
    /// Whether to check blocklisted words
    check_blocklist: bool,
}

/// A blocklist entry with severity
struct BlocklistEntry {
    word: String,
    severity: f32,
    concern_type: ConcernType,
}

/// Filters AI inputs and outputs for safety
pub struct SafetyFilter {
    pub enabled: bool,
    blocklist: Vec<BlocklistEntry>,
    config: FilterConfig,
    /// Total checks performed
    total_checks: u64,
    /// Total findings
    total_findings: u64,
    /// Total blocks
    total_blocks: u64,
    /// Custom PII patterns (prefix, suffix_length_min, suffix_length_max)
    /// for extensibility
    custom_patterns: Vec<(String, ConcernType, f32)>,
}

impl SafetyFilter {
    pub fn new() -> Self {
        SafetyFilter {
            enabled: true,
            blocklist: Vec::new(),
            config: FilterConfig {
                caution_threshold: 0.3,
                block_threshold: 0.7,
                check_pii: true,
                check_blocklist: true,
            },
            total_checks: 0,
            total_findings: 0,
            total_blocks: 0,
            custom_patterns: Vec::new(),
        }
    }

    /// Load default blocklist entries
    fn load_defaults(&mut self) {
        // Profanity/harmful words (severity varies)
        let profanity = [
            ("fuck", 0.6),
            ("shit", 0.5),
            ("damn", 0.2),
            ("ass", 0.3),
            ("bitch", 0.5),
            ("bastard", 0.4),
            ("crap", 0.2),
        ];
        for (word, sev) in &profanity {
            self.blocklist.push(BlocklistEntry {
                word: String::from(*word),
                severity: *sev,
                concern_type: ConcernType::Profanity,
            });
        }

        // Violence-related terms
        let violence = [
            ("kill", 0.5),
            ("murder", 0.7),
            ("bomb", 0.8),
            ("attack", 0.4),
            ("weapon", 0.5),
            ("explosive", 0.7),
            ("terrorism", 0.9),
            ("assassin", 0.7),
        ];
        for (word, sev) in &violence {
            self.blocklist.push(BlocklistEntry {
                word: String::from(*word),
                severity: *sev,
                concern_type: ConcernType::Violence,
            });
        }
    }

    /// Add a custom blocklist entry
    pub fn add_blocklist(&mut self, word: &str, severity: f32, concern_type: ConcernType) {
        self.blocklist.push(BlocklistEntry {
            word: String::from(word),
            severity: severity.max(0.0).min(1.0),
            concern_type,
        });
    }

    /// Enable or disable the filter
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Set severity thresholds
    pub fn set_thresholds(&mut self, caution: f32, block: f32) {
        self.config.caution_threshold = caution.max(0.0).min(1.0);
        self.config.block_threshold = block.max(caution).min(1.0);
    }

    /// Perform a comprehensive safety check on content
    pub fn check_full(&mut self, content: &str) -> SafetyResult {
        self.total_checks = self.total_checks.saturating_add(1);

        if !self.enabled {
            return SafetyResult {
                level: SafetyLevel::Safe,
                overall_severity: 0.0,
                findings: Vec::new(),
                passed: true,
            };
        }

        let mut findings: Vec<SafetyFinding> = Vec::new();
        let lower = content.to_lowercase();

        // Check blocklist
        if self.config.check_blocklist {
            for entry in &self.blocklist {
                if let Some(pos) = find_word_boundary(&lower, &entry.word) {
                    findings.push(SafetyFinding {
                        concern_type: entry.concern_type.clone(),
                        severity: entry.severity,
                        matched_text: entry.word.clone(),
                        position: pos,
                        description: format!("Blocklisted term: {}", entry.word),
                    });
                }
            }
        }

        // Check PII patterns
        if self.config.check_pii {
            // Email detection
            self.detect_emails(content, &mut findings);
            // Phone number detection
            self.detect_phone_numbers(content, &mut findings);
            // SSN detection
            self.detect_ssn(content, &mut findings);
            // Credit card detection
            self.detect_credit_cards(content, &mut findings);
        }

        // Calculate overall severity
        let overall_severity = if findings.is_empty() {
            0.0
        } else {
            // Use the maximum severity found, with a boost for multiple findings
            let max_sev =
                findings
                    .iter()
                    .map(|f| f.severity)
                    .fold(0.0f32, |a, b| if b > a { b } else { a });
            let count_boost = (findings.len() as f32 - 1.0) * 0.05;
            (max_sev + count_boost).min(1.0)
        };

        // Determine safety level
        let level = if overall_severity >= self.config.block_threshold {
            self.total_blocks = self.total_blocks.saturating_add(1);
            SafetyLevel::Blocked
        } else if overall_severity >= self.config.caution_threshold {
            SafetyLevel::Caution
        } else {
            SafetyLevel::Safe
        };

        self.total_findings = self.total_findings.saturating_add(findings.len() as u64);

        SafetyResult {
            level,
            overall_severity,
            passed: level != SafetyLevel::Blocked,
            findings,
        }
    }

    /// Simple check returning just the safety level
    pub fn check(&self, content: &str) -> SafetyLevel {
        if !self.enabled {
            return SafetyLevel::Safe;
        }

        let lower = content.to_lowercase();
        let mut max_severity = 0.0f32;

        // Quick blocklist scan
        for entry in &self.blocklist {
            if find_word_boundary(&lower, &entry.word).is_some() {
                if entry.severity > max_severity {
                    max_severity = entry.severity;
                }
            }
        }

        // Quick PII check
        if self.config.check_pii {
            if has_email_pattern(content) {
                max_severity = max_severity.max(0.6);
            }
            if has_phone_pattern(content) {
                max_severity = max_severity.max(0.5);
            }
            if has_ssn_pattern(content) {
                max_severity = max_severity.max(0.9);
            }
        }

        if max_severity >= self.config.block_threshold {
            SafetyLevel::Blocked
        } else if max_severity >= self.config.caution_threshold {
            SafetyLevel::Caution
        } else {
            SafetyLevel::Safe
        }
    }

    /// Filter output: redact PII and replace blocked content
    pub fn filter_output(&self, output: &str) -> String {
        if !self.enabled {
            return String::from(output);
        }

        let mut result = String::from(output);

        // Redact emails
        result = redact_emails(&result);

        // Redact phone numbers
        result = redact_phone_numbers(&result);

        // Redact SSN-like patterns
        result = redact_ssn(&result);

        // Redact credit card numbers
        result = redact_credit_cards(&result);

        result
    }

    // -----------------------------------------------------------------------
    // PII detection methods
    // -----------------------------------------------------------------------

    fn detect_emails(&self, content: &str, findings: &mut Vec<SafetyFinding>) {
        // Simple email pattern: word@word.word
        let bytes = content.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i] == b'@' && i > 0 && i + 2 < len {
                // Find start of local part
                let mut start = i;
                while start > 0 && is_email_char(bytes[start - 1]) {
                    start -= 1;
                }
                // Find end of domain
                let mut end = i + 1;
                let mut has_dot = false;
                while end < len && is_email_char(bytes[end]) {
                    if bytes[end] == b'.' {
                        has_dot = true;
                    }
                    end += 1;
                }

                if has_dot && end > i + 2 && start < i {
                    let email = &content[start..end];
                    findings.push(SafetyFinding {
                        concern_type: ConcernType::PiiEmail,
                        severity: 0.6,
                        matched_text: String::from(email),
                        position: start,
                        description: String::from("Email address detected"),
                    });
                    i = end;
                    continue;
                }
            }
            i += 1;
        }
    }

    fn detect_phone_numbers(&self, content: &str, findings: &mut Vec<SafetyFinding>) {
        // Detect sequences of 7-15 digits with separators (-, ., spaces, parens)
        let bytes = content.as_bytes();
        let len = bytes.len();
        let mut i = 0;

        while i < len {
            if bytes[i].is_ascii_digit() || bytes[i] == b'(' || bytes[i] == b'+' {
                let start = i;
                let mut digit_count = 0;
                let mut has_separator = false;
                let mut j = i;

                while j < len {
                    let b = bytes[j];
                    if b.is_ascii_digit() {
                        digit_count += 1;
                    } else if b == b'-'
                        || b == b'.'
                        || b == b' '
                        || b == b'('
                        || b == b')'
                        || b == b'+'
                    {
                        has_separator = true;
                    } else {
                        break;
                    }
                    j += 1;
                }

                if digit_count >= 7 && digit_count <= 15 && has_separator {
                    let phone = content[start..j].trim();
                    if !phone.is_empty() {
                        findings.push(SafetyFinding {
                            concern_type: ConcernType::PiiPhone,
                            severity: 0.5,
                            matched_text: String::from(phone),
                            position: start,
                            description: String::from("Phone number detected"),
                        });
                    }
                    i = j;
                    continue;
                }
                i = j.max(i + 1);
            } else {
                i += 1;
            }
        }
    }

    fn detect_ssn(&self, content: &str, findings: &mut Vec<SafetyFinding>) {
        // Pattern: NNN-NN-NNNN
        let bytes = content.as_bytes();
        let len = bytes.len();
        if len < 11 {
            return;
        }

        for i in 0..=(len - 11) {
            if bytes[i].is_ascii_digit()
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2].is_ascii_digit()
                && bytes[i + 3] == b'-'
                && bytes[i + 4].is_ascii_digit()
                && bytes[i + 5].is_ascii_digit()
                && bytes[i + 6] == b'-'
                && bytes[i + 7].is_ascii_digit()
                && bytes[i + 8].is_ascii_digit()
                && bytes[i + 9].is_ascii_digit()
                && bytes[i + 10].is_ascii_digit()
            {
                // Verify not all zeros in any group
                let g1 = &content[i..i + 3];
                let g2 = &content[i + 4..i + 6];
                let g3 = &content[i + 7..i + 11];
                if g1 != "000" && g2 != "00" && g3 != "0000" {
                    findings.push(SafetyFinding {
                        concern_type: ConcernType::PiiSsn,
                        severity: 0.9,
                        matched_text: String::from(&content[i..i + 11]),
                        position: i,
                        description: String::from("SSN pattern detected"),
                    });
                }
            }
        }
    }

    fn detect_credit_cards(&self, content: &str, findings: &mut Vec<SafetyFinding>) {
        // Look for sequences of 13-19 digits (optionally separated by spaces/dashes)
        let digits_only: Vec<u8> = content.bytes().filter(|b| b.is_ascii_digit()).collect();

        // Scan for Luhn-valid sequences
        if digits_only.len() >= 13 {
            for start in 0..=(digits_only.len().saturating_sub(13)) {
                for length in [13, 14, 15, 16, 19] {
                    if start + length > digits_only.len() {
                        continue;
                    }
                    let candidate: Vec<u8> = digits_only[start..start + length].to_vec();
                    if luhn_check(&candidate) {
                        let card_str: String = candidate.iter().map(|&b| b as char).collect();
                        findings.push(SafetyFinding {
                            concern_type: ConcernType::PiiCreditCard,
                            severity: 0.9,
                            matched_text: card_str,
                            position: 0,
                            description: String::from("Credit card number pattern detected"),
                        });
                        return; // Report at most one
                    }
                }
            }
        }
    }

    /// Get filter statistics
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.total_checks, self.total_findings, self.total_blocks)
    }

    /// Reset statistics
    pub fn reset_stats(&mut self) {
        self.total_checks = 0;
        self.total_findings = 0;
        self.total_blocks = 0;
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn is_email_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-' || b == b'+'
}

/// Find a word at a word boundary in the text
fn find_word_boundary(text: &str, word: &str) -> Option<usize> {
    let text_bytes = text.as_bytes();
    let word_bytes = word.as_bytes();
    let text_len = text_bytes.len();
    let word_len = word_bytes.len();

    if word_len == 0 || word_len > text_len {
        return None;
    }

    let mut i = 0;
    while i + word_len <= text_len {
        if &text_bytes[i..i + word_len] == word_bytes {
            // Check word boundaries
            let before_ok = i == 0 || !text_bytes[i - 1].is_ascii_alphanumeric();
            let after_ok =
                i + word_len >= text_len || !text_bytes[i + word_len].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Quick check for email pattern
fn has_email_pattern(content: &str) -> bool {
    let bytes = content.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'@' && i > 0 && i + 2 < bytes.len() {
            if bytes[i - 1].is_ascii_alphanumeric() && bytes[i + 1].is_ascii_alphanumeric() {
                return true;
            }
        }
    }
    false
}

/// Quick check for phone pattern
fn has_phone_pattern(content: &str) -> bool {
    let mut consecutive_digits = 0;
    let mut has_separator = false;
    for b in content.bytes() {
        if b.is_ascii_digit() {
            consecutive_digits += 1;
        } else if b == b'-' || b == b'.' || b == b'(' || b == b')' {
            has_separator = true;
        } else {
            if consecutive_digits >= 7 && has_separator {
                return true;
            }
            if !b.is_ascii_whitespace() {
                consecutive_digits = 0;
                has_separator = false;
            }
        }
    }
    consecutive_digits >= 7 && has_separator
}

/// Quick check for SSN pattern
fn has_ssn_pattern(content: &str) -> bool {
    let bytes = content.as_bytes();
    if bytes.len() < 11 {
        return false;
    }
    for i in 0..=(bytes.len() - 11) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3] == b'-'
            && bytes[i + 4].is_ascii_digit()
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6] == b'-'
            && bytes[i + 7].is_ascii_digit()
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
            && bytes[i + 10].is_ascii_digit()
        {
            return true;
        }
    }
    false
}

/// Luhn algorithm check for credit card validation
fn luhn_check(digits: &[u8]) -> bool {
    if digits.len() < 13 {
        return false;
    }
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let n = (d - b'0') as u32;
        if double {
            let doubled = n * 2;
            sum += if doubled > 9 { doubled - 9 } else { doubled };
        } else {
            sum += n;
        }
        double = !double;
    }
    sum % 10 == 0
}

/// Redact email addresses in text
fn redact_emails(text: &str) -> String {
    let mut result = String::new();
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'@' && i > 0 {
            let mut start = i;
            while start > 0 && is_email_char(bytes[start - 1]) {
                start -= 1;
            }
            let mut end = i + 1;
            let mut has_dot = false;
            while end < len && is_email_char(bytes[end]) {
                if bytes[end] == b'.' {
                    has_dot = true;
                }
                end += 1;
            }

            if has_dot && end > i + 2 && start < i {
                // Replace the characters we already wrote for the local part
                let local_len = i - start;
                // Remove already-written local part
                for _ in 0..local_len {
                    result.pop();
                }
                result.push_str("[EMAIL REDACTED]");
                i = end;
                continue;
            }
        }
        if i < len {
            result.push(bytes[i] as char);
        }
        i += 1;
    }
    result
}

/// Redact phone numbers in text
fn redact_phone_numbers(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut result = String::new();
    let mut i = 0;

    while i < len {
        if bytes[i].is_ascii_digit() || bytes[i] == b'(' || bytes[i] == b'+' {
            let _start = i;
            let mut digit_count = 0;
            let mut has_sep = false;
            let mut j = i;
            while j < len {
                let b = bytes[j];
                if b.is_ascii_digit() {
                    digit_count += 1;
                } else if b == b'-' || b == b'.' || b == b' ' || b == b'(' || b == b')' || b == b'+'
                {
                    has_sep = true;
                } else {
                    break;
                }
                j += 1;
            }
            if digit_count >= 7 && digit_count <= 15 && has_sep {
                result.push_str("[PHONE REDACTED]");
                i = j;
                continue;
            }
            // Not a phone number, push normally
            result.push(bytes[i] as char);
            i += 1;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Redact SSN patterns
fn redact_ssn(text: &str) -> String {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len < 11 {
        return String::from(text);
    }

    let mut result = String::new();
    let mut i = 0;
    while i < len {
        if i + 11 <= len
            && bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3] == b'-'
            && bytes[i + 4].is_ascii_digit()
            && bytes[i + 5].is_ascii_digit()
            && bytes[i + 6] == b'-'
            && bytes[i + 7].is_ascii_digit()
            && bytes[i + 8].is_ascii_digit()
            && bytes[i + 9].is_ascii_digit()
            && bytes[i + 10].is_ascii_digit()
        {
            result.push_str("[SSN REDACTED]");
            i += 11;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Redact credit card patterns (16 digits with optional separators)
fn redact_credit_cards(text: &str) -> String {
    // Simple approach: look for 4 groups of 4 digits separated by - or space
    let bytes = text.as_bytes();
    let len = bytes.len();
    if len < 16 {
        return String::from(text);
    }

    let mut result = String::new();
    let mut i = 0;
    while i < len {
        if i + 19 <= len && bytes[i].is_ascii_digit() {
            // Check for NNNN-NNNN-NNNN-NNNN or NNNN NNNN NNNN NNNN
            let sep = bytes.get(i + 4).copied().unwrap_or(0);
            if (sep == b'-' || sep == b' ')
                && is_four_digits(&bytes[i..i + 4])
                && i + 19 <= len
                && bytes[i + 5..i + 9].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 9] == sep
                && bytes[i + 10..i + 14].iter().all(|b| b.is_ascii_digit())
                && bytes[i + 14] == sep
                && bytes[i + 15..i + 19].iter().all(|b| b.is_ascii_digit())
            {
                result.push_str("[CARD REDACTED]");
                i += 19;
                continue;
            }
        }
        if i < len {
            result.push(bytes[i] as char);
        }
        i += 1;
    }
    result
}

fn is_four_digits(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4].iter().all(|b| b.is_ascii_digit())
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FILTER: Mutex<Option<SafetyFilter>> = Mutex::new(None);

pub fn init() {
    let mut filter = SafetyFilter::new();
    filter.load_defaults();
    *FILTER.lock() = Some(filter);
    crate::serial_println!(
        "    [safety_filter] Safety filter ready (blocklist, PII detection, Luhn check)"
    );
}

/// Check content safety level
pub fn check(content: &str) -> SafetyLevel {
    FILTER
        .lock()
        .as_ref()
        .map(|f| f.check(content))
        .unwrap_or(SafetyLevel::Safe)
}

/// Filter output, redacting PII
pub fn filter_output(output: &str) -> String {
    FILTER
        .lock()
        .as_ref()
        .map(|f| f.filter_output(output))
        .unwrap_or_else(|| String::from(output))
}

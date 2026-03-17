use crate::sync::Mutex;
/// Comprehensive PDF analysis for solicitations
///
/// Part of the Bid Command AIOS app. Extracts requirements,
/// scope, deliverables, and cost estimates from bid documents.
/// Uses local heuristic text analysis (no external AI APIs).
use alloc::string::String;
use alloc::vec::Vec;

/// Structured result from solicitation analysis
pub struct AnalysisResult {
    pub summary: String,
    pub key_requirements: Vec<String>,
    pub deliverables: Vec<String>,
    pub estimated_cost_low: u64,
    pub estimated_cost_high: u64,
}

pub struct SolicitationAnalyzer {
    pub bid_id: u64,
}

/// Helper: extract ASCII text from raw PDF-like data.
/// In a real kernel we would have a full PDF parser; here we extract
/// printable ASCII runs as a simplified approach for raw document bytes.
fn extract_text(data: &[u8]) -> String {
    let mut text = String::new();
    for &b in data {
        if b >= 0x20 && b < 0x7F {
            text.push(b as char);
        } else if b == b'\n' || b == b'\r' || b == b'\t' {
            text.push(' ');
        }
    }
    text
}

/// Helper: check if a text segment contains a keyword (case-insensitive).
fn contains_keyword(text: &str, keyword: &str) -> bool {
    if keyword.is_empty() || text.len() < keyword.len() {
        return false;
    }
    let text_bytes = text.as_bytes();
    let kw_bytes = keyword.as_bytes();
    let kw_len = kw_bytes.len();

    for i in 0..=(text_bytes.len() - kw_len) {
        let mut matched = true;
        for j in 0..kw_len {
            let a = if text_bytes[i + j].is_ascii_uppercase() {
                text_bytes[i + j] + 32
            } else {
                text_bytes[i + j]
            };
            let b = if kw_bytes[j].is_ascii_uppercase() {
                kw_bytes[j] + 32
            } else {
                kw_bytes[j]
            };
            if a != b {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

/// Requirement keyword patterns
const REQUIREMENT_KEYWORDS: &[&str] = &[
    "shall",
    "must",
    "required",
    "mandatory",
    "comply",
    "provide",
    "ensure",
    "deliver",
    "perform",
    "maintain",
];

/// Deliverable keyword patterns
const DELIVERABLE_KEYWORDS: &[&str] = &[
    "deliverable",
    "report",
    "plan",
    "schedule",
    "document",
    "submission",
    "drawing",
    "specification",
    "manual",
    "training",
];

/// Extract sentences containing any of the given keywords
fn extract_sentences_with_keywords(
    text: &str,
    keywords: &[&str],
    max_results: usize,
) -> Vec<String> {
    let mut results = Vec::new();
    // Split on periods to get rough "sentences"
    let bytes = text.as_bytes();
    let mut start = 0;
    let len = bytes.len();

    for i in 0..len {
        if bytes[i] == b'.' || i == len - 1 {
            let end = if i == len - 1 { len } else { i + 1 };
            if end > start + 5 {
                let sentence = &text[start..end];
                // Trim leading whitespace
                let trimmed = sentence.trim_start();
                if !trimmed.is_empty() {
                    for &kw in keywords {
                        if contains_keyword(trimmed, kw) {
                            // Truncate long sentences
                            let s = if trimmed.len() > 200 {
                                let mut s = String::new();
                                for c in trimmed[..200].chars() {
                                    s.push(c);
                                }
                                s.push_str("...");
                                s
                            } else {
                                let mut s = String::new();
                                for c in trimmed.chars() {
                                    s.push(c);
                                }
                                s
                            };
                            results.push(s);
                            break;
                        }
                    }
                }
                if results.len() >= max_results {
                    break;
                }
            }
            start = i + 1;
        }
    }
    results
}

/// Heuristic cost estimation based on document size and keyword density
fn estimate_cost(text: &str) -> (u64, u64) {
    let word_count = text.split(' ').filter(|w| !w.is_empty()).count() as u64;

    // Simple heuristic: larger documents tend to indicate larger projects
    // Base cost per significant word, with multipliers for complexity keywords
    let base = word_count * 50; // 50 cents per word as base
    let complexity_keywords = &["complex", "large", "multi", "phase", "year", "million"];
    let mut multiplier: u64 = 100; // 100 = 1.0x
    for &kw in complexity_keywords {
        if contains_keyword(text, kw) {
            multiplier += 25; // +25% per complexity keyword
        }
    }

    let low = (base * multiplier) / 100;
    let high = (low * 150) / 100; // High estimate is 1.5x low

    // Clamp to reasonable ranges (at least $1000, at most $10M)
    let low_clamped = if low < 100_000 {
        100_000
    } else if low > 1_000_000_000 {
        1_000_000_000
    } else {
        low
    };
    let high_clamped = if high < 150_000 {
        150_000
    } else if high > 1_500_000_000 {
        1_500_000_000
    } else {
        high
    };

    (low_clamped, high_clamped)
}

impl SolicitationAnalyzer {
    pub fn new(bid_id: u64) -> Self {
        crate::serial_println!("    [analysis] analyzer created for bid {}", bid_id);
        Self { bid_id }
    }

    /// Run comprehensive analysis on document bytes.
    /// Extracts text, identifies requirements, deliverables, and estimates costs.
    pub fn analyze(&self, pdf_data: &[u8]) -> Result<AnalysisResult, ()> {
        if pdf_data.is_empty() {
            crate::serial_println!("    [analysis] bid {}: empty document data", self.bid_id);
            return Err(());
        }

        crate::serial_println!(
            "    [analysis] bid {}: analyzing {} bytes of document data",
            self.bid_id,
            pdf_data.len()
        );

        let text = extract_text(pdf_data);
        if text.len() < 10 {
            crate::serial_println!(
                "    [analysis] bid {}: insufficient extractable text",
                self.bid_id
            );
            return Err(());
        }

        let key_requirements = extract_sentences_with_keywords(&text, REQUIREMENT_KEYWORDS, 20);
        let deliverables = extract_sentences_with_keywords(&text, DELIVERABLE_KEYWORDS, 15);
        let (cost_low, cost_high) = estimate_cost(&text);

        // Build summary
        let mut summary = String::from("Solicitation analysis: ");
        let req_count_str = format_u64(key_requirements.len() as u64);
        let del_count_str = format_u64(deliverables.len() as u64);
        summary.push_str(&req_count_str);
        summary.push_str(" requirements, ");
        summary.push_str(&del_count_str);
        summary.push_str(" deliverables identified. Estimated cost range: $");
        summary.push_str(&format_u64(cost_low / 100));
        summary.push_str(" - $");
        summary.push_str(&format_u64(cost_high / 100));

        crate::serial_println!(
            "    [analysis] bid {}: {} requirements, {} deliverables found",
            self.bid_id,
            key_requirements.len(),
            deliverables.len()
        );

        Ok(AnalysisResult {
            summary,
            key_requirements,
            deliverables,
            estimated_cost_low: cost_low,
            estimated_cost_high: cost_high,
        })
    }

    /// Extract red flags and risks from an analysis result.
    /// Looks for patterns that indicate potential problems.
    pub fn detect_risks(&self, result: &AnalysisResult) -> Vec<String> {
        let mut risks = Vec::new();

        // Check for high complexity (many requirements)
        if result.key_requirements.len() > 15 {
            risks.push(String::from(
                "High complexity: large number of requirements (>15) increases delivery risk",
            ));
        }

        // Check for high cost spread
        if result.estimated_cost_high > 0 {
            let spread = ((result.estimated_cost_high - result.estimated_cost_low) * 100)
                / result.estimated_cost_high;
            if spread > 40 {
                risks.push(String::from(
                    "Cost uncertainty: wide estimate range indicates unclear scope",
                ));
            }
        }

        // Check for many deliverables
        if result.deliverables.len() > 10 {
            risks.push(String::from(
                "Many deliverables: high documentation burden may affect timeline",
            ));
        }

        // Check for no deliverables (might be missing info)
        if result.deliverables.is_empty() {
            risks.push(String::from(
                "No deliverables identified: solicitation may be incomplete or non-standard format",
            ));
        }

        // Check for keywords in requirements that indicate risk
        let risk_keywords = &["liquidated damages", "penalty", "termination", "default"];
        for &kw in risk_keywords {
            for req in &result.key_requirements {
                if contains_keyword(req.as_str(), kw) {
                    let mut risk = String::from("Contract risk: '");
                    risk.push_str(kw);
                    risk.push_str("' clause detected in requirements");
                    risks.push(risk);
                    break;
                }
            }
        }

        crate::serial_println!(
            "    [analysis] bid {}: {} risks detected",
            self.bid_id,
            risks.len()
        );
        risks
    }
}

/// Simple u64 to string formatter (no alloc::format! dependency for numbers)
fn format_u64(mut n: u64) -> String {
    if n == 0 {
        return String::from("0");
    }
    let mut digits = Vec::new();
    while n > 0 {
        digits.push((b'0' + (n % 10) as u8) as char);
        n /= 10;
    }
    digits.reverse();
    let mut s = String::new();
    for c in digits {
        s.push(c);
    }
    s
}

/// Global analyzer state
static ANALYSIS_READY: Mutex<bool> = Mutex::new(false);

pub fn init() {
    let mut ready = ANALYSIS_READY.lock();
    *ready = true;
    crate::serial_println!("    [analysis] solicitation analysis pipeline registered");
}

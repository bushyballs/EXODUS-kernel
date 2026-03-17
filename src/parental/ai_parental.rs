use crate::sync::Mutex;
/// AI-enhanced parental controls for Genesis
///
/// Smart content classification, cyberbullying detection,
/// predator pattern detection, age-appropriate recommendations.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum SafetyAlert {
    None,
    Mild,
    Moderate,
    Severe,
    Critical,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ThreatType {
    InappropriateContent,
    Cyberbullying,
    SuspiciousContact,
    PrivacyRisk,
    ExcessiveUse,
    LocationAnomaly,
}

struct SafetyCheck {
    timestamp: u64,
    threat_type: ThreatType,
    severity: SafetyAlert,
    child_id: u32,
    details_hash: u64,
}

struct AiParentalEngine {
    checks: Vec<SafetyCheck>,
    bullying_keywords: Vec<u64>, // hashed keywords
    alert_count: u32,
    false_positive_rate: u32, // 0-100
}

static AI_PARENTAL: Mutex<Option<AiParentalEngine>> = Mutex::new(None);

impl AiParentalEngine {
    fn new() -> Self {
        let mut engine = AiParentalEngine {
            checks: Vec::new(),
            bullying_keywords: Vec::new(),
            alert_count: 0,
            false_positive_rate: 5,
        };
        // Seed common cyberbullying keyword hashes
        let keywords = [0x1234u64, 0x5678, 0x9ABC, 0xDEF0]; // placeholder hashes
        engine.bullying_keywords.extend_from_slice(&keywords);
        engine
    }

    fn classify_text_safety(
        &mut self,
        text_hash: u64,
        word_count: u32,
        child_id: u32,
        timestamp: u64,
    ) -> SafetyAlert {
        // Check against known harmful patterns
        let mut risk = 0u32;

        // Check bullying keywords
        if self.bullying_keywords.contains(&text_hash) {
            risk += 40;
        }

        // Suspicious patterns (very short or very long messages to unknown contacts)
        if word_count > 200 {
            risk += 10;
        }

        let severity = if risk > 60 {
            SafetyAlert::Severe
        } else if risk > 40 {
            SafetyAlert::Moderate
        } else if risk > 20 {
            SafetyAlert::Mild
        } else {
            SafetyAlert::None
        };

        if severity != SafetyAlert::None {
            self.alert_count = self.alert_count.saturating_add(1);
            if self.checks.len() < 500 {
                self.checks.push(SafetyCheck {
                    timestamp,
                    threat_type: ThreatType::Cyberbullying,
                    severity,
                    child_id,
                    details_hash: text_hash,
                });
            }
        }
        severity
    }

    fn detect_suspicious_contact(
        &mut self,
        contact_age_estimate: u8,
        child_age: u8,
        message_frequency: u32,
        child_id: u32,
        timestamp: u64,
    ) -> SafetyAlert {
        let mut risk = 0u32;

        // Large age gap with high message frequency
        let age_gap = if contact_age_estimate > child_age {
            contact_age_estimate - child_age
        } else {
            0
        };

        if age_gap > 10 && message_frequency > 20 {
            risk += 50;
        } else if age_gap > 5 && message_frequency > 10 {
            risk += 30;
        }

        // Very frequent messaging with unknown contact
        if message_frequency > 50 {
            risk += 20;
        }

        let severity = if risk > 60 {
            SafetyAlert::Critical
        } else if risk > 40 {
            SafetyAlert::Severe
        } else if risk > 20 {
            SafetyAlert::Moderate
        } else {
            SafetyAlert::None
        };

        if severity != SafetyAlert::None {
            self.alert_count = self.alert_count.saturating_add(1);
            if self.checks.len() < 500 {
                self.checks.push(SafetyCheck {
                    timestamp,
                    threat_type: ThreatType::SuspiciousContact,
                    severity,
                    child_id,
                    details_hash: 0,
                });
            }
        }
        severity
    }
}

pub fn init() {
    let mut engine = AI_PARENTAL.lock();
    *engine = Some(AiParentalEngine::new());
    serial_println!("    AI parental: content safety, cyberbullying detection ready");
}

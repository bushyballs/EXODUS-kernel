use crate::sync::Mutex;
/// Call screening for Genesis
///
/// Spam detection, caller ID, block list,
/// robocall filtering, business verification.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum CallerCategory {
    Known,
    Unknown,
    Spam,
    Robocall,
    Scam,
    Business,
    Emergency,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ScreenAction {
    Allow,
    Block,
    SendToVoicemail,
    SilentRing,
    WarnUser,
}

struct BlockedNumber {
    number: [u8; 20],
    number_len: usize,
    reason: CallerCategory,
    blocked_at: u64,
}

struct SpamPattern {
    prefix: [u8; 10],
    prefix_len: usize,
    score: u32, // 0-100
    report_count: u32,
}

struct ScreeningEngine {
    blocked: Vec<BlockedNumber>,
    spam_patterns: Vec<SpamPattern>,
    spam_calls_blocked: u32,
    total_screened: u32,
    allow_unknown: bool,
}

static SCREENING: Mutex<Option<ScreeningEngine>> = Mutex::new(None);

impl ScreeningEngine {
    fn new() -> Self {
        ScreeningEngine {
            blocked: Vec::new(),
            spam_patterns: Vec::new(),
            spam_calls_blocked: 0,
            total_screened: 0,
            allow_unknown: true,
        }
    }

    fn screen_call(&mut self, number: &[u8]) -> (CallerCategory, ScreenAction) {
        self.total_screened = self.total_screened.saturating_add(1);

        // Check block list
        for b in &self.blocked {
            if &b.number[..b.number_len] == number {
                self.spam_calls_blocked = self.spam_calls_blocked.saturating_add(1);
                return (b.reason, ScreenAction::Block);
            }
        }

        // Check spam patterns
        for pattern in &self.spam_patterns {
            let plen = pattern.prefix_len;
            if number.len() >= plen && &number[..plen] == &pattern.prefix[..plen] {
                if pattern.score > 80 {
                    self.spam_calls_blocked = self.spam_calls_blocked.saturating_add(1);
                    return (CallerCategory::Spam, ScreenAction::Block);
                } else if pattern.score > 50 {
                    return (CallerCategory::Spam, ScreenAction::WarnUser);
                }
            }
        }

        // Check for known robocall patterns
        // Short numbers with high frequency are suspicious
        if number.len() < 7 && number.len() > 3 {
            return (CallerCategory::Unknown, ScreenAction::WarnUser);
        }

        if !self.allow_unknown {
            return (CallerCategory::Unknown, ScreenAction::SendToVoicemail);
        }

        (CallerCategory::Unknown, ScreenAction::Allow)
    }

    fn block_number(&mut self, number: &[u8], reason: CallerCategory, timestamp: u64) {
        let mut num = [0u8; 20];
        let len = number.len().min(20);
        num[..len].copy_from_slice(&number[..len]);
        self.blocked.push(BlockedNumber {
            number: num,
            number_len: len,
            reason,
            blocked_at: timestamp,
        });
    }

    fn report_spam(&mut self, number: &[u8]) {
        // Extract prefix (area code + exchange)
        if number.len() >= 6 {
            let plen = 6;
            let mut prefix = [0u8; 10];
            prefix[..plen].copy_from_slice(&number[..plen]);
            if let Some(p) = self
                .spam_patterns
                .iter_mut()
                .find(|p| p.prefix_len == plen && p.prefix[..plen] == prefix[..plen])
            {
                p.report_count = p.report_count.saturating_add(1);
                p.score = (p.score + 10).min(100);
            } else if self.spam_patterns.len() < 200 {
                self.spam_patterns.push(SpamPattern {
                    prefix,
                    prefix_len: plen,
                    score: 30,
                    report_count: 1,
                });
            }
        }
    }
}

pub fn init() {
    let mut engine = SCREENING.lock();
    *engine = Some(ScreeningEngine::new());
    serial_println!("    Telephony: call screening (spam, robocall filter) ready");
}

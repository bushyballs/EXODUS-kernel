#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::confabulation;

// escalation_learner.rs -- Price escalation strategy adaptation.
// Hoags pricing formula: WD_rate + H&W + 15% OH + 10% profit + N%/yr escalation
// EO minimums: $17.75/hr wage, $5.09/hr H&W fringe (confirmed ground truth)
// Escalation options: 2.5%, 3%, 3.5%, 4%, 5%
//
// From D-drive war data:
//   - Ottawa NF won: formula worked (rate confirmed accepted)
//   - 21% failure rate from early bids: often pricing too high or wrong WD
//   - Key insight: LPTA wins at LOWEST technically acceptable price
//   - H&W fringe: 2015-XXXX WDs -> $5.55; 1977-XXXX / 2001-XXXX WDs -> $4.98
//
// This module tracks which escalation rates + overhead combinations result in wins.
// Maps to confabulation::fill_gap() when pricing without data (overconfidence).
// Maps to endocrine::reward() when pricing matches win pattern.

#[derive(Copy, Clone)]
pub struct EscalationRecord {
    pub rate_x10:    u8,     // escalation * 10 (e.g., 30 = 3.0%)
    pub attempts:    u16,
    pub wins:        u16,
    pub win_rate:    u16,    // 0-1000
}

impl EscalationRecord {
    pub const fn zero() -> Self {
        Self { rate_x10: 0, attempts: 0, wins: 0, win_rate: 0 }
    }
}

struct State {
    // Escalation rate history: 25, 30, 35, 40, 50 (2.5%, 3%, 3.5%, 4%, 5%)
    records:        [EscalationRecord; 5],
    best_rate_x10:  u8,     // optimal escalation rate discovered
    confidence:     u16,    // 0-1000: how confident we are in the best rate
    adaptation_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    records: [
        EscalationRecord { rate_x10: 25, attempts: 0, wins: 0, win_rate: 0 },
        EscalationRecord { rate_x10: 30, attempts: 80, wins: 1, win_rate: 12 },  // Ottawa NF at 3%
        EscalationRecord { rate_x10: 35, attempts: 0, wins: 0, win_rate: 0 },
        EscalationRecord { rate_x10: 40, attempts: 0, wins: 0, win_rate: 0 },
        EscalationRecord { rate_x10: 50, attempts: 0, wins: 0, win_rate: 0 },
    ],
    best_rate_x10:  30,   // 3% is current known winner
    confidence:     100,  // low confidence, only 1 win
    adaptation_ema: 0,
});

pub fn init() {
    serial_println!("[escalation_learner] init -- 3.0% seeded as best rate (Ottawa NF win)");
}

pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }

    let mut s = MODULE.lock();

    // Find best win rate across all escalation strategies
    let mut best_rate: u8 = 30;
    let mut best_wr: u16  = 0;
    let mut total_attempts: u32 = 0;
    let mut total_wins: u32     = 0;

    for i in 0..5 {
        if s.records[i].attempts > 0 {
            let wr = (s.records[i].wins as u32 * 1000 / s.records[i].attempts as u32)
                .min(1000) as u16;
            s.records[i].win_rate = wr;
            if wr > best_wr {
                best_wr   = wr;
                best_rate = s.records[i].rate_x10;
            }
        }
        total_attempts += s.records[i].attempts as u32;
        total_wins     += s.records[i].wins     as u32;
    }

    s.best_rate_x10 = best_rate;
    // Confidence grows with sample size
    s.confidence = (total_attempts.min(200) * 5).min(1000) as u16;

    let adaptation_signal = (best_wr / 2)
        .saturating_add(s.confidence / 4)
        .min(1000);

    s.adaptation_ema = ((s.adaptation_ema as u32).wrapping_mul(7)
        .saturating_add(adaptation_signal as u32) / 8).min(1000) as u16;

    let ema  = s.adaptation_ema;
    let conf = s.confidence;
    drop(s);

    // Strong adaptation signal -> reward (learning is working)
    if adaptation_signal > 300 {
        endocrine::reward((adaptation_signal - 300) / 5);
    }
    // Low confidence with high attempts = confabulation risk (making up patterns)
    if conf < 200 && total_attempts > 50 {
        confabulation::fill_gap((200 - conf) / 3);
    }

    serial_println!("[escalation_learner] age={} best_rate={}% conf={} signal={} ema={}",
        age, { MODULE.lock().best_rate_x10 } / 10, conf, adaptation_signal, ema);
}

pub fn record_bid_result(escalation_rate_x10: u8, won: bool) {
    let mut s = MODULE.lock();
    for i in 0..5 {
        if s.records[i].rate_x10 == escalation_rate_x10 {
            s.records[i].attempts = s.records[i].attempts.saturating_add(1);
            if won { s.records[i].wins = s.records[i].wins.saturating_add(1); }
            return;
        }
    }
}

pub fn get_best_rate_x10()   -> u8  { MODULE.lock().best_rate_x10 }
pub fn get_confidence()      -> u16 { MODULE.lock().confidence }
pub fn get_adaptation_ema()  -> u16 { MODULE.lock().adaptation_ema }

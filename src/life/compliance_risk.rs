#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::immune;
use super::endocrine;

// compliance_risk.rs -- Solicitation buried-requirement detection -> immune vigilance.
// Federal solicitations bury compliance traps deep in their pages:
//   - Page 8: "requires active pesticide applicator license"
//   - Page 12: "must self-perform, no subcontracting"
//   - Page 15: "Davis-Bacon applies" (construction wages, not SCA)
//   - Page 3: "procurement method is PIEE portal only" (not email)
//
// From D-drive war data analysis: 4 bids rejected for wrong channel (PIEE vs email).
// Multiple non-responsive due to SF1449 form issues buried in solicitation.
// HIGH PAGE COUNT + LOW HEALTH = high buried compliance risk = immune alert.
//
// Maps to immune.defend() -- the organism detects hidden threats.
// High risk -> heightened vigilance to read EVERY PAGE carefully.

const MAX_BIDS: usize = 8;

#[derive(Copy, Clone)]
struct BidRisk {
    sol_hash:      u32,
    page_count:    u16,    // total solicitation pages
    health_score:  u16,    // 0-1000 bid health
    risk_level:    u16,    // 0-1000 compliance risk
}

impl BidRisk {
    const fn zero() -> Self {
        Self { sol_hash: 0, page_count: 0, health_score: 1000, risk_level: 0 }
    }
}

struct State {
    bids:           [BidRisk; MAX_BIDS],
    avg_risk:       u16,    // 0-1000 composite
    known_traps:    u16,    // count of known compliance traps (grows with learning)
    risk_ema:       u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids:        [BidRisk::zero(); MAX_BIDS],
    avg_risk:    0,
    known_traps: 12,    // known from D-drive: PIEE trap (4), SF1449 (4), form type (4)
    risk_ema:    0,
});

// Risk curve: more pages = more opportunity for buried requirements
fn page_risk(pages: u16) -> u16 {
    match pages {
        0..=5   => 100,
        6..=10  => 200,
        11..=20 => 400,
        21..=40 => 600,
        41..=80 => 800,
        _       => 1000,
    }
}

pub fn init() {
    serial_println!("[compliance_risk] init -- 12 known traps from D-drive (PIEE,SF1449,form-type)");
    // Seed known trap pattern: W-prefix = PIEE required (not email)
    // BigBend W9128F26QA015 was emailed when it should have been PIEE -- documented trap
    // Davis-Bacon vs SCA: SF-1442 = construction ($60+/hr), not SCA service
}

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let mut s = MODULE.lock();
    let mut total_risk: u32 = 0;
    let mut active: u32 = 0;

    for i in 0..MAX_BIDS {
        if s.bids[i].sol_hash != 0 {
            let base   = page_risk(s.bids[i].page_count);
            // Low health score = likely has buried compliance issues
            let health_penalty = if s.bids[i].health_score < 900 {
                ((900 - s.bids[i].health_score) * 500) / 900
            } else { 0 };
            let risk = (base as u32).saturating_add(health_penalty as u32).min(1000) as u16;
            s.bids[i].risk_level = risk;
            total_risk += risk as u32;
            active += 1;
        }
    }

    let avg_risk = if active > 0 { (total_risk / active).min(1000) as u16 } else { 200 };
    s.risk_ema = ((s.risk_ema as u32).wrapping_mul(7)
        .saturating_add(avg_risk as u32) / 8).min(1000) as u16;
    s.avg_risk = avg_risk;
    let traps = s.known_traps;
    let ema   = s.risk_ema;
    drop(s);

    // High compliance risk -> immune defense (organism senses hidden threat)
    if avg_risk > 400 {
        immune::defend((avg_risk - 400) / 4);
    }
    // Known traps growing -> healthy learning (mild reward for detection)
    if traps > 20 {
        endocrine::reward(10);
    }

    serial_println!("[compliance_risk] age={} avg_risk={} known_traps={} ema={}",
        age, avg_risk, traps, ema);
}

pub fn register_bid(hash: u32, pages: u16, health: u16) {
    let mut s = MODULE.lock();
    for i in 0..MAX_BIDS {
        if s.bids[i].sol_hash == hash || s.bids[i].sol_hash == 0 {
            s.bids[i] = BidRisk {
                sol_hash: hash, page_count: pages,
                health_score: health, risk_level: page_risk(pages),
            };
            return;
        }
    }
}

pub fn found_compliance_trap() {
    let mut s = MODULE.lock();
    s.known_traps = s.known_traps.saturating_add(1);
}

pub fn get_avg_risk()    -> u16 { MODULE.lock().avg_risk }
pub fn get_risk_ema()    -> u16 { MODULE.lock().risk_ema }
pub fn get_known_traps() -> u16 { MODULE.lock().known_traps }

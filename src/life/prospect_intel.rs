#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::memory_hierarchy;

// prospect_intel.rs -- CO interaction history -> award probability prediction.
// Maps to memory_hierarchy (we remember COs) + endocrine.reward (when prediction is right).
//
// Seeded from 80 sent bids, 100% CO ack rate, 1 confirmed win (Ottawa NF).
// D-drive war data: Pascal Carter, Joshua Hope, Tina Frankenbery all confirmed receipt.
// Key predictor: COs who ask clarifying questions = higher award probability.
// COs who reply with "proposal accepted" immediately = LPTA win signal.
//
// CO Tier Classification (from bid history):
//   TIER_1 (Responsive + awarded before): highest award probability
//   TIER_2 (Responsive, acknowledgement only): moderate probability
//   TIER_3 (No reply or auto-ack): low probability

#[derive(Copy, Clone, PartialEq)]
pub enum CoTier {
    Tier1,   // responded + interacted deeply
    Tier2,   // acknowledged, no further contact
    Tier3,   // auto-ack or no response
}

const MAX_CO_SLOTS: usize = 16;

struct CoRecord {
    hash:        u32,     // CO identifier hash
    tier:        CoTier,
    bids_sent:   u8,
    awards:      u8,
    award_prob:  u16,     // 0-1000
}

impl CoRecord {
    const fn zero() -> Self {
        Self { hash: 0, tier: CoTier::Tier3, bids_sent: 0, awards: 0, award_prob: 0 }
    }
}

struct State {
    cos:              [CoRecord; MAX_CO_SLOTS],
    avg_award_prob:   u16,    // 0-1000 composite
    intel_ema:        u16,
    prediction_depth: u16,    // 0-1000: how confident we are in predictions
}

static MODULE: Mutex<State> = Mutex::new(State {
    cos:              [CoRecord::zero(); MAX_CO_SLOTS],
    avg_award_prob:   125,    // 1/80 = 1.25% base win rate -> 12 signal pts; seeded at ~125
    intel_ema:        0,
    prediction_depth: 200,    // early learning phase
});

pub fn init() {
    serial_println!("[prospect_intel] init -- 80 CO contacts, 1 award, seeding tier database");
    // Seed known high-value COs from D-drive war data
    // Pascal Carter (USACE W912WJ) -- responsive, LPTA, confirmed receipt
    seed_co(0x50435254, CoTier::Tier2, 3, 0);
    // Joshua Hope (USACE W912BV) -- responsive, confirmed receipt
    seed_co(0x4A534850, CoTier::Tier2, 2, 0);
    // Tina Frankenbery (USDA 1240LP/1240BG) -- responsive, Ottawa NF WINNER
    seed_co(0x54494E41, CoTier::Tier1, 4, 1);   // 1 award = Ottawa NF
    // Jennifer Olson (USDA 1240BG) -- confirmed receipt
    seed_co(0x4A454E4F, CoTier::Tier2, 2, 0);
    // Sally Leitch (DOI 140P14) -- confirmed receipt
    seed_co(0x53414C4C, CoTier::Tier2, 1, 0);
}

fn seed_co(hash: u32, tier: CoTier, sent: u8, awards: u8) {
    let mut s = MODULE.lock();
    for i in 0..MAX_CO_SLOTS {
        if s.cos[i].hash == 0 {
            let prob: u16 = match tier {
                CoTier::Tier1 => if sent > 0 { (awards as u32 * 1000 / sent as u32) as u16 } else { 500 },
                CoTier::Tier2 => 80,
                CoTier::Tier3 => 20,
            };
            s.cos[i] = CoRecord { hash, tier, bids_sent: sent, awards, award_prob: prob };
            return;
        }
    }
}

pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }

    let mut s = MODULE.lock();
    // Compute avg award probability across known COs
    let mut total_prob: u32 = 0;
    let mut active: u32 = 0;
    for i in 0..MAX_CO_SLOTS {
        if s.cos[i].hash != 0 {
            total_prob += s.cos[i].award_prob as u32;
            active += 1;
        }
    }
    let avg_prob = if active > 0 { (total_prob / active).min(1000) as u16 } else { 0 };

    // Prediction depth grows with number of COs tracked
    s.prediction_depth = (active as u32 * 15).min(1000) as u16;

    s.intel_ema = ((s.intel_ema as u32).wrapping_mul(7)
        .saturating_add(avg_prob as u32) / 8).min(1000) as u16;
    s.avg_award_prob = avg_prob;
    let ema = s.intel_ema;
    let depth = s.prediction_depth;
    drop(s);

    // Strong predictive intel -> memory encoding (we know our opponents)
    if depth > 400 {
        memory_hierarchy::encode(depth / 10);
    }
    // High award probability pipeline -> dopamine anticipation
    if avg_prob > 300 {
        endocrine::reward((avg_prob - 300) / 5);
    }

    serial_println!("[prospect_intel] age={} cos={} avg_prob={} depth={} ema={}",
        age, active, avg_prob, depth, ema);
}

pub fn record_award(co_hash: u32) {
    let mut s = MODULE.lock();
    for i in 0..MAX_CO_SLOTS {
        if s.cos[i].hash == co_hash {
            s.cos[i].awards = s.cos[i].awards.saturating_add(1);
            s.cos[i].tier   = CoTier::Tier1;
            let sent = s.cos[i].bids_sent.max(1);
            s.cos[i].award_prob = (s.cos[i].awards as u32 * 1000 / sent as u32).min(1000) as u16;
            return;
        }
    }
}

pub fn get_avg_award_prob()   -> u16 { MODULE.lock().avg_award_prob }
pub fn get_intel_ema()        -> u16 { MODULE.lock().intel_ema }
pub fn get_prediction_depth() -> u16 { MODULE.lock().prediction_depth }

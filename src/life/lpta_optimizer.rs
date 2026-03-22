#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;

// lpta_optimizer.rs -- LPTA price pressure -> efficiency tradeoff signal.
// LPTA = Lowest Price Technically Acceptable.
// In LPTA: win by being cheapest while still technically passing.
// Pricing too high = guaranteed loss. Pricing too low = lose money.
//
// The sweet spot is the FLOOR: WD_wage + fringe + 15% OH + 10% profit.
// Going below floor = unsustainable (stress). At floor = optimal (calm + reward).
// Above floor by >20% = high loss probability (anxiety signal).
//
// From D-drive war data:
//   Ottawa NF formula: confirmed accepted at floor + 3% escalation
//   9 bad bids: several priced too high (above 20% floor margin)
//   LPTA fraction: ~70% of Hoags' active pipeline
//
// Maps to: endocrine (stress when over-priced, reward at floor), tracks margin health.

struct State {
    bids_lpta:          u32,    // count of LPTA bids in pipeline
    bids_at_floor:      u32,    // bids priced at formula floor
    bids_above_margin:  u32,    // bids priced >20% above floor (loss risk)
    efficiency_signal:  u16,    // 0-1000
    efficiency_ema:     u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids_lpta:         158,    // 70% of 226 active bids are LPTA
    bids_at_floor:      80,    // sent bids: assumed at floor
    bids_above_margin:   9,    // 9 bad bids: priced out of range
    efficiency_signal:   0,
    efficiency_ema:      0,
});

pub fn init() {
    serial_println!("[lpta_optimizer] init -- 158 LPTA bids, 70% pipeline, floor formula confirmed");
}

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let mut s = MODULE.lock();
    let total = s.bids_lpta.max(1);

    // Efficiency: fraction of LPTA bids priced at the optimal floor
    let at_floor_ratio = ((s.bids_at_floor as u32 * 1000) / total).min(1000) as u16;
    // Penalty: bids above margin (will lose on LPTA evaluation)
    let margin_penalty = ((s.bids_above_margin as u32 * 200) / total).min(500) as u16;

    let efficiency = at_floor_ratio.saturating_sub(margin_penalty).min(1000);
    s.efficiency_ema = ((s.efficiency_ema as u32).wrapping_mul(7)
        .saturating_add(efficiency as u32) / 8).min(1000) as u16;
    s.efficiency_signal = efficiency;
    let ema = s.efficiency_ema;
    let above = s.bids_above_margin;
    drop(s);

    // Good floor pricing -> reward (we're competitive)
    if efficiency > 600 {
        endocrine::reward((efficiency - 600) / 6);
    }
    // Over-priced bids -> stress (we're leaving wins on the table)
    if above > 5 {
        endocrine::stress((above as u16 * 20).min(300));
    }

    serial_println!("[lpta_optimizer] age={} efficiency={} above_margin={} ema={}",
        age, efficiency, above, ema);
}

pub fn bid_at_floor() {
    let mut s = MODULE.lock();
    s.bids_at_floor = s.bids_at_floor.saturating_add(1);
}

pub fn bid_above_margin() {
    let mut s = MODULE.lock();
    s.bids_above_margin = s.bids_above_margin.saturating_add(1);
}

pub fn get_efficiency_signal() -> u16 { MODULE.lock().efficiency_signal }
pub fn get_efficiency_ema()    -> u16 { MODULE.lock().efficiency_ema }

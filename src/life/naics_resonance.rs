#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::memory_hierarchy;

// naics_resonance.rs -- NAICS code specialization depth -> expertise signal.
// Hoags has 5 primary NAICS codes. Mastery in each code = different resonance.
// More bids per code = deeper expertise = stronger signal = more reward.
//
// NAICS portfolio (from D-drive research):
//   561720 Janitorial      ~35% of bids -> primary specialty
//   561730 Grounds/Landscaping ~30%    -> secondary specialty
//   562111 Solid Waste     ~15%        -> tertiary
//   562991 Septic/Other    ~10%        -> niche
//   561210 Facilities      ~8%         -> emerging
//
// Maps to memory_hierarchy (domain knowledge depth) + endocrine (expertise confidence).

struct NaicsSlot {
    code:        u32,
    bid_count:   u16,
    win_count:   u16,
    mastery:     u16,    // 0-1000: expertise depth in this code
}

impl NaicsSlot {
    const fn new(code: u32, bids: u16) -> Self {
        Self { code, bid_count: bids, win_count: 0, mastery: 0 }
    }
}

struct State {
    slots:         [NaicsSlot; 5],
    primary_code:  u32,    // highest-mastery NAICS
    portfolio_ema: u16,    // 0-1000 composite mastery signal
}

static MODULE: Mutex<State> = Mutex::new(State {
    slots: [
        NaicsSlot::new(561720, 79),   // Janitorial: 79 bids (35% of 226)
        NaicsSlot::new(561730, 68),   // Grounds: 68 bids (30%)
        NaicsSlot::new(562111, 34),   // Solid Waste: 34 bids (15%)
        NaicsSlot::new(562991, 23),   // Septic: 23 bids (10%)
        NaicsSlot::new(561210, 18),   // Facilities: 18 bids (8%)
    ],
    primary_code:  561720,
    portfolio_ema: 0,
});

// Mastery curve: bids sent in a code -> expertise depth
fn bids_to_mastery(bids: u16, wins: u16) -> u16 {
    let volume  = (bids as u32 * 8).min(600) as u16;
    let win_exp = (wins as u32 * 200).min(400) as u16;
    volume.saturating_add(win_exp).min(1000)
}

pub fn init() {
    serial_println!("[naics_resonance] init -- 5 NAICS codes, 561720 primary (79 bids)");
}

pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }

    let mut s = MODULE.lock();
    let mut best_mastery: u16 = 0;
    let mut best_code:    u32 = 561720;
    let mut total_mastery: u32 = 0;

    for i in 0..5 {
        let m = bids_to_mastery(s.slots[i].bid_count, s.slots[i].win_count);
        s.slots[i].mastery = m;
        total_mastery += m as u32;
        if m > best_mastery {
            best_mastery = m;
            best_code    = s.slots[i].code;
        }
    }

    s.primary_code = best_code;
    let portfolio_signal = (total_mastery / 5).min(1000) as u16;
    s.portfolio_ema = ((s.portfolio_ema as u32).wrapping_mul(7)
        .saturating_add(portfolio_signal as u32) / 8).min(1000) as u16;
    let ema = s.portfolio_ema;
    drop(s);

    // Deep expertise -> reward (competence = dopamine)
    if portfolio_signal > 400 {
        endocrine::reward((portfolio_signal - 400) / 6);
    }
    // Primary specialty mastery -> memory encoding (we know this domain)
    if best_mastery > 500 {
        memory_hierarchy::encode(best_mastery / 10);
    }

    serial_println!("[naics_resonance] age={} primary={} mastery={} portfolio={} ema={}",
        age, best_code, best_mastery, portfolio_signal, ema);
}

pub fn record_win_in_naics(code: u32) {
    let mut s = MODULE.lock();
    for i in 0..5 {
        if s.slots[i].code == code {
            s.slots[i].win_count = s.slots[i].win_count.saturating_add(1);
            return;
        }
    }
}

pub fn get_portfolio_ema() -> u16 { MODULE.lock().portfolio_ema }
pub fn get_primary_code()  -> u32 { MODULE.lock().primary_code }

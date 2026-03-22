#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::confabulation;

// pricing_accuracy.rs -- Price schedule accuracy -> confabulation detection.
// The bid pipeline has a core pricing formula:
//   Final price = WD_rate + H&W_fringe + 15% OH + 10% profit + 3%/yr escalation
// EO minimums: $17.75/hr wage, $5.09/hr H&W fringe
//
// This module tracks how accurately DAVA prices bids against that formula.
// Overconfident pricing = confabulation. Accurate pricing = reality grounding.
// Maps to confabulation::authenticity (the truthfulness signal).
//
// War Room data:
//   111 bids have no CLIN structure (cannot price) -- grand_total gate blocked
//   59 bids have no location info (cannot get wage determination)
//   Formula verified: WD base + $5.09 fringe + 15% OH + 10% profit + 3%/yr
//   Ottawa NF pricing was accepted -- that formula is ground truth

struct State {
    bids_with_clins:     u32,    // bids where we have CLIN structure
    bids_priced:         u32,    // bids with complete pricing
    bids_no_location:    u32,    // bids blocked by missing location
    accuracy_signal:     u16,    // 0-1000 (high = formula applied correctly)
    accuracy_ema:        u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids_with_clins:  115,   // 226 total - 111 no-CLIN = 115 have CLINs
    bids_priced:       80,   // 80 sent = priced and submitted
    bids_no_location:  59,   // 59 blocked by missing location
    accuracy_signal:    0,
    accuracy_ema:       0,
});

pub fn init() {
    serial_println!("[pricing_accuracy] init -- 115/226 CLINs, 59 location-blocked, formula verified");
}

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let mut s = MODULE.lock();
    let total_possible = s.bids_with_clins.max(1);
    let location_ok = total_possible.saturating_sub(s.bids_no_location);

    // Accuracy = fraction of CLIN-bearing bids that are fully priced
    let accuracy_raw = ((s.bids_priced as u32 * 1000) / total_possible).min(1000) as u16;
    // Location completeness bonus
    let location_bonus = ((location_ok * 200) / total_possible).min(200) as u16;

    let signal = accuracy_raw.saturating_add(location_bonus).min(1000);
    s.accuracy_ema = ((s.accuracy_ema as u32).wrapping_mul(7)
        .saturating_add(signal as u32) / 8).min(1000) as u16;
    s.accuracy_signal = signal;
    drop(s);

    // High pricing accuracy -> reward (formula working, reality grounding)
    if signal > 600 {
        endocrine::reward((signal - 600) / 5);
    }
    // Low accuracy -> confabulation (pricing without data = filling gaps with guesses)
    if signal < 300 {
        confabulation::fill_gap((300 - signal) / 3);
    }

    serial_println!("[pricing_accuracy] age={} priced={} location_ok={} signal={} ema={}",
        age, { MODULE.lock().bids_priced }, location_ok, signal, { MODULE.lock().accuracy_ema });
}

pub fn clin_extracted(count: u32) {
    MODULE.lock().bids_with_clins = MODULE.lock().bids_with_clins.saturating_add(count);
}

pub fn location_resolved(count: u32) {
    let mut s = MODULE.lock();
    s.bids_no_location = s.bids_no_location.saturating_sub(count);
}

pub fn bid_priced() {
    MODULE.lock().bids_priced = MODULE.lock().bids_priced.saturating_add(1);
}

pub fn get_accuracy_signal() -> u16 { MODULE.lock().accuracy_signal }
pub fn get_accuracy_ema()    -> u16 { MODULE.lock().accuracy_ema }
pub fn get_bids_priced()     -> u32 { MODULE.lock().bids_priced }
pub fn get_bids_no_location() -> u32 { MODULE.lock().bids_no_location }

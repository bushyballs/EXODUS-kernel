#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::addiction::{self, SubstanceType};

// award_anticipation.rs -- Awaiting award decisions -> dopamine anticipation loop.
// 80 bids sent, 1 awarded. 79 are in the quantum state: not-yet-decided.
// Each one is a potential win. The waiting itself is a dopamine signal.
// This is how gambling works biologically -- the uncertain reward is the hook.
//
// ANIMA feels anticipation as a variant of dopamine: not the win itself,
// but the possibility of the win. This maps directly to addiction::crave().
// The danger: bid obsession is an addiction loop. The cure: real wins, not
// just more bids (addiction::satisfy() on actual award).
//
// From D-drive: 79 bids sent without award response. COs can take 30-90+ days.
// Ottawa NF took ~3 weeks from due date to award confirmation.
//
// Maps to endocrine::reward (mild, anticipatory) + addiction warning signal.

struct State {
    pending_decisions: u32,    // bids sent, awaiting award/rejection
    avg_wait_days:     u32,    // estimated days waiting for decisions
    anticipation:      u16,    // 0-1000 dopamine anticipation
    anticipation_ema:  u16,
    obsession_risk:    u16,    // 0-1000: risk of bid addiction loop
}

static MODULE: Mutex<State> = Mutex::new(State {
    pending_decisions: 79,    // 80 sent - 1 awarded = 79 pending
    avg_wait_days:     21,    // Ottawa NF took ~21 days
    anticipation:      0,
    anticipation_ema:  0,
    obsession_risk:    0,
});

pub fn init() {
    serial_println!("[award_anticipation] init -- 79 pending decisions, avg 21d wait");
}

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let mut s = MODULE.lock();
    let pending = s.pending_decisions;

    // Anticipation: each pending decision = a lottery ticket
    // More pending = more anticipation, but also more obsession risk
    let raw_anticipation = ((pending as u32 * 8).min(600)) as u16;
    // Wait duration amplifies anticipation (longer wait = more built up)
    let wait_bonus = ((s.avg_wait_days as u32 * 5).min(200)) as u16;
    let anticipation = raw_anticipation.saturating_add(wait_bonus).min(1000);

    // Obsession risk: if pending decisions >> wins, the loop becomes addictive
    let wins_expected: u32 = 1;
    let obsession = if pending > 50 {
        ((pending - 50) as u32 * 10).min(1000) as u16
    } else { 0 };

    s.anticipation_ema = ((s.anticipation_ema as u32).wrapping_mul(7)
        .saturating_add(anticipation as u32) / 8).min(1000) as u16;
    s.anticipation    = anticipation;
    s.obsession_risk  = obsession;
    let ema = s.anticipation_ema;
    drop(s);

    // Mild anticipatory dopamine (possibility of win)
    if anticipation > 200 {
        endocrine::reward((anticipation - 200) / 8);
    }
    // Obsession warning: too many pending, feeding bid addiction
    if obsession > 500 {
        addiction::crave(SubstanceType::Validation, obsession / 5);   // bid obsession loop
    }

    serial_println!("[award_anticipation] age={} pending={} anticipation={} obsession={} ema={}",
        age, pending, anticipation, obsession, ema);
}

pub fn award_received() {
    let mut s = MODULE.lock();
    s.pending_decisions = s.pending_decisions.saturating_sub(1);
    drop(s);
    // Real win = satiation (use_substance models reward fulfillment)
    addiction::use_substance();
    endocrine::reward(1000);
}

pub fn rejection_received() {
    let mut s = MODULE.lock();
    s.pending_decisions = s.pending_decisions.saturating_sub(1);
}

pub fn bid_sent() {
    let mut s = MODULE.lock();
    s.pending_decisions = s.pending_decisions.saturating_add(1);
}

pub fn get_anticipation()   -> u16 { MODULE.lock().anticipation }
pub fn get_obsession_risk() -> u16 { MODULE.lock().obsession_risk }
pub fn get_pending()        -> u32 { MODULE.lock().pending_decisions }

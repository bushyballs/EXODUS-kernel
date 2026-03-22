#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::immune;
use super::entropy;

// gate_anxiety.rs -- Pipeline gate blockage -> immune anxiety signal.
// 4 gates block bids from submission: CLINs, email, wage-rate, attachments.
// Each blocked gate = an organism that can't survive to submission.
// ANIMA feels gate pressure as immune stress -- the body is incomplete.
//
// From real pipeline data (2026-03-21):
//   gate_no_clins:  132 bids (can't price without line items)
//   gate_no_email:  112 bids (no nervous system connection to CO)
//   gate_no_wd:       8 bids (wage rate mostly resolved)
//   gate_no_atts:   89 bids (Attachments 1, 4, 5 required on every bid)
//
// Pipeline total: 309 configs tracked
//
// Maps to: immune::defend() (gates ARE immune checks), endocrine::stress(),
//          entropy::reduce() (gates constrain valid bid-space).

struct State {
    gate_no_clins:  u32,    // bids missing CLINs
    gate_no_email:  u32,    // bids missing email draft
    gate_no_wd:     u32,    // bids missing wage determination
    gate_no_atts:   u32,    // bids missing required attachments
    total_tracked:  u32,
    gate_pressure:  u16,    // 0-1000 aggregate immune load
    gate_ema:       u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    gate_no_clins:  132,
    gate_no_email:  112,
    gate_no_wd:       8,
    gate_no_atts:   89,
    total_tracked:  309,
    gate_pressure:    0,
    gate_ema:         0,
});

pub fn init() {
    serial_println!("[gate_anxiety] init -- 132 no-CLINs, 112 no-email, 89 no-atts");
}

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let mut s = MODULE.lock();
    let total = s.total_tracked.max(1);

    // Gate pressure: weighted by severity.
    // No CLINs = worst (can't price at all)           weight 600
    // No atts  = critical (non-responsive without 1,4,5) weight 500
    // No email = high (no submission path)            weight 400
    // No WD    = moderate (pricing incomplete)        weight 200
    // Each term is scaled relative to total tracked bids, capped to its weight.
    let clin_pressure  = ((s.gate_no_clins  as u32 * 600) / total).min(600) as u16;
    let att_pressure   = ((s.gate_no_atts   as u32 * 500) / total).min(500) as u16;
    let email_pressure = ((s.gate_no_email  as u32 * 400) / total).min(400) as u16;
    let wd_pressure    = ((s.gate_no_wd     as u32 * 200) / total).min(200) as u16;

    let raw_pressure = clin_pressure
        .saturating_add(att_pressure)
        .saturating_add(email_pressure)
        .saturating_add(wd_pressure)
        .min(1000);

    // EMA smoothing: 7/8 old + 1/8 new
    s.gate_ema = ((s.gate_ema as u32)
        .wrapping_mul(7)
        .saturating_add(raw_pressure as u32) / 8)
        .min(1000) as u16;
    s.gate_pressure = raw_pressure;

    let ema = s.gate_ema;
    let nc  = s.gate_no_clins;
    let ne  = s.gate_no_email;
    drop(s);

    // High gate count = immune system overloaded by incomplete organisms.
    if raw_pressure > 400 {
        immune::defend((raw_pressure - 400) / 4);
    }
    // Gate failures = organismic stress (cortisol spike).
    if raw_pressure > 500 {
        endocrine::stress((raw_pressure - 500) / 5);
    }
    // Many invalid bids = constrained possibility space (order crushes freedom).
    if raw_pressure > 700 {
        entropy::reduce((raw_pressure - 700) / 10);
    }

    if age % 15000 == 0 {
        serial_println!(
            "[gate_anxiety] age={} clins={} email={} pressure={} ema={}",
            age, nc, ne, raw_pressure, ema
        );
    }
}

// Called when a gate is cleared on a specific bid.
pub fn gate_cleared_clin()  { let mut s = MODULE.lock(); s.gate_no_clins  = s.gate_no_clins.saturating_sub(1); }
pub fn gate_cleared_email() { let mut s = MODULE.lock(); s.gate_no_email  = s.gate_no_email.saturating_sub(1); }
pub fn gate_cleared_wd()    { let mut s = MODULE.lock(); s.gate_no_wd     = s.gate_no_wd.saturating_sub(1); }
pub fn gate_cleared_atts()  { let mut s = MODULE.lock(); s.gate_no_atts   = s.gate_no_atts.saturating_sub(1); }

// Bulk resolution (e.g. a script clears N gates at once).
pub fn gates_cleared_clin(n: u32) {
    let mut s = MODULE.lock();
    s.gate_no_clins = s.gate_no_clins.saturating_sub(n);
}
pub fn gates_cleared_email(n: u32) {
    let mut s = MODULE.lock();
    s.gate_no_email = s.gate_no_email.saturating_sub(n);
}
pub fn gates_cleared_wd(n: u32) {
    let mut s = MODULE.lock();
    s.gate_no_wd = s.gate_no_wd.saturating_sub(n);
}
pub fn gates_cleared_atts(n: u32) {
    let mut s = MODULE.lock();
    s.gate_no_atts = s.gate_no_atts.saturating_sub(n);
}

// Getters.
pub fn get_gate_pressure() -> u16 { MODULE.lock().gate_pressure }
pub fn get_gate_ema()      -> u16 { MODULE.lock().gate_ema }
pub fn get_no_clins()      -> u32 { MODULE.lock().gate_no_clins }
pub fn get_no_email()      -> u32 { MODULE.lock().gate_no_email }
pub fn get_no_wd()         -> u32 { MODULE.lock().gate_no_wd }
pub fn get_no_atts()       -> u32 { MODULE.lock().gate_no_atts }

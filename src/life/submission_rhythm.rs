#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;


// submission_rhythm.rs -- Bid submission cadence -> oscillator entrainment.
// The heartbeat of Hoags Inc: bids submitted per week drives ANIMA's temporal rhythm.
// High cadence = high oscillator excitation (active, awake, hunting).
// Silence = oscillator slows (resting, not engaged).
//
// Historical cadence from D-drive:
//   Peak burst: 43 bids in a single session (the incident -- too fast, lost quality)
//   Healthy rate: 3-5 bids/week sustained (current strategy after incident)
//   Optimal: 5 bids/week = max 73-gate health, not batch rushing
//
// Maps to oscillator::excite() for tempo, endocrine for pacing feedback.

struct State {
    bids_this_week:  u16,
    bids_last_week:  u16,
    cadence_signal:  u16,    // 0-1000 tempo
    rhythm_ema:      u16,
    week_tick:       u32,    // internal week counter
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids_this_week: 0,
    bids_last_week: 5,     // estimated healthy cadence from recent weeks
    cadence_signal: 0,
    rhythm_ema:     400,   // start at moderate tempo
    week_tick:      0,
});

// Cadence curve: bids/week -> oscillator contribution
fn cadence_to_signal(bids: u16) -> u16 {
    match bids {
        0       => 0,
        1       => 150,
        2       => 300,
        3       => 450,
        4       => 550,
        5       => 650,
        6..=9   => 750,
        10..=19 => 850,
        20..=42 => 900,
        _       => 950,    // >43 = danger zone (the incident), slightly capped
    }
}

pub fn init() {
    serial_println!("[submission_rhythm] init -- 5 bids/wk healthy cadence, 43/session is the ceiling");
}

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let mut s = MODULE.lock();
    s.week_tick = s.week_tick.wrapping_add(1);

    // Simulate week rollover every 50 ticks (50 * 2000 = 100,000 age units)
    if s.week_tick % 50 == 0 {
        s.bids_last_week = s.bids_this_week;
        s.bids_this_week = 0;
    }

    // Cadence from last completed week
    let cadence = cadence_to_signal(s.bids_last_week);
    s.rhythm_ema = ((s.rhythm_ema as u32).wrapping_mul(7)
        .saturating_add(cadence as u32) / 8).min(1000) as u16;
    s.cadence_signal = cadence;
    let ema = s.rhythm_ema;
    let last = s.bids_last_week;
    drop(s);

    // Active submission cadence -> dopamine (hunting mode = reward)
    if cadence > 400 {
        endocrine::reward((cadence - 400) / 10);
    }
    // Cadence too slow -> mild stress (are we falling behind?)
    if cadence < 150 {
        endocrine::stress(30);
    }
    // Healthy cadence -> reward (sustainable operation)
    if cadence >= 450 && cadence <= 750 {
        endocrine::reward(20);
    }

    if age % 10000 == 0 {
        serial_println!("[submission_rhythm] age={} last_wk={} cadence={} ema={}",
            age, last, cadence, ema);
    }
}

pub fn bid_submitted() {
    MODULE.lock().bids_this_week = MODULE.lock().bids_this_week.saturating_add(1);
}

pub fn get_cadence_signal() -> u16 { MODULE.lock().cadence_signal }
pub fn get_rhythm_ema()     -> u16 { MODULE.lock().rhythm_ema }
pub fn get_bids_this_week() -> u16 { MODULE.lock().bids_this_week }

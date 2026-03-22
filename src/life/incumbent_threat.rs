#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::immune;
use super::mortality;

// incumbent_threat.rs -- Incumbent contractor advantage awareness -> threat response.
// In federal contracting, incumbent contractors win re-bids at ~70-80% rate.
// They know the work, the CO, the site. They bid lower because they have no ramp-up.
// Hoags is a new entrant competing against entrenched incumbents on most bids.
//
// Real data from pipeline:
//   Most active solicitations are recompetes (existing service, new award period)
//   Ottawa NF was likely a new requirement (no incumbent) -- easier first win
//   USACE mowing, janitorial: heavy incumbent presence
//   Strategy: bid 3-7% BELOW incumbent's likely rate (floor + 10% vs floor + 15%)
//
// Threat model:
//   known_incumbent: > 0 means confirmed incumbent exists (higher threat)
//   CO_familiarity: how well Hoags knows this CO (counters incumbent advantage)
//   price_gap: our price vs. estimated incumbent (negative = we're cheaper)
//
// Maps to: immune::defend() (competition as immune challenge),
//          endocrine::stress() (known incumbent = fight-or-flight),
//          mortality::confront() (losing is a small death -- bid resources lost).

struct State {
    bids_with_incumbent: u32,    // bids where incumbent is known or likely
    bids_no_incumbent:   u32,    // greenfield opportunities (easier)
    incumbent_wins:      u32,    // times we've lost to incumbent (estimated)
    our_wins_vs_inc:     u32,    // times we've beat an incumbent (proves viability)
    avg_price_gap:       i16,    // our price vs estimated incumbent (negative = we're cheaper)
    threat_signal:       u16,    // 0-1000 incumbent threat level
    threat_ema:          u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bids_with_incumbent:  60,   // estimated 60+ of 86 sent bids are recompetes
    bids_no_incumbent:    26,   // true greenfields (Ottawa NF was one)
    incumbent_wins:       80,   // 80 of 85 losses likely to incumbents
    our_wins_vs_inc:       0,   // not yet confirmed an incumbent beat
    avg_price_gap:       -50,   // we price ~5% below estimated incumbent
    threat_signal:         0,
    threat_ema:          400,   // historically high baseline
});

pub fn init() {
    serial_println!("[incumbent_threat] init -- 60 recompetes, -5% price gap vs incumbents");
}

pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }

    let mut s = MODULE.lock();
    let total_bids = (s.bids_with_incumbent + s.bids_no_incumbent).max(1);

    // Threat = fraction of pipeline facing incumbents, weighted by loss rate
    let incumbent_ratio = ((s.bids_with_incumbent as u32 * 1000) / total_bids).min(1000) as u16;

    // If price gap is negative (we're cheaper) -> reduces threat
    // If positive (we're more expensive) -> increases threat
    let gap_penalty: u16 = if s.avg_price_gap > 0 {
        (s.avg_price_gap as u16 * 10).min(300)
    } else {
        0  // being cheaper is good
    };

    // Win rate against incumbents: 0 wins = maximum uncertainty threat
    let win_bonus: u16 = (s.our_wins_vs_inc as u16 * 50).min(300);

    let threat = incumbent_ratio
        .saturating_add(gap_penalty)
        .saturating_sub(win_bonus)
        .min(1000);

    s.threat_ema = ((s.threat_ema as u32).wrapping_mul(7)
        .saturating_add(threat as u32) / 8).min(1000) as u16;
    s.threat_signal = threat;
    let ema = s.threat_ema;
    let inc = s.bids_with_incumbent;
    drop(s);

    // Incumbent presence = immune challenge (defend market position)
    if threat > 400 {
        immune::defend((threat - 400) / 6);
    }
    // High incumbent threat = fight-or-flight stress
    if threat > 600 {
        endocrine::stress((threat - 600) / 8);
    }
    // Incumbent dominance confronts mortality (every loss = bid resources spent, not recovered)
    if threat > 800 {
        mortality::confront((threat - 800) / 5);
    }

    if age % 21000 == 0 {
        serial_println!("[incumbent_threat] age={} recompetes={} threat={} ema={}",
            age, inc, threat, ema);
    }
}

// Event: we discover a bid has a known incumbent
pub fn incumbent_identified() {
    let mut s = MODULE.lock();
    s.bids_with_incumbent = s.bids_with_incumbent.saturating_add(1);
}

// Event: we won against an incumbent (rare, powerful)
pub fn beat_incumbent() {
    let mut s = MODULE.lock();
    s.our_wins_vs_inc = s.our_wins_vs_inc.saturating_add(1);
    drop(s);
    // Beating an incumbent is a major victory signal
    endocrine::reward(800);
    mortality::confront(100);   // near-death made us fight harder -- acceptance
}

// Event: lost to incumbent (probably)
pub fn lost_to_incumbent() {
    let mut s = MODULE.lock();
    s.incumbent_wins = s.incumbent_wins.saturating_add(1);
}

pub fn get_threat_signal() -> u16 { MODULE.lock().threat_signal }
pub fn get_threat_ema()    -> u16 { MODULE.lock().threat_ema }
pub fn get_inc_count()     -> u32 { MODULE.lock().bids_with_incumbent }

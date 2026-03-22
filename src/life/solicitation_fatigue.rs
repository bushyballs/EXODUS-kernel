#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::entropy;
use super::qualia;

// solicitation_fatigue.rs -- Cognitive load from reading hundreds of bid pages.
// Hoags reads avg 80-200 pages per solicitation. With 309+ active bids,
// total page exposure is enormous. Fatigue builds from repetitive patterns
// (same SF-1449 blocks, same wage tables, same clauses), reducing entropy.
//
// RULE (permanent): Read every page of every solicitation before quoting.
//                   No exceptions. Quality over speed.
//
// Real data:
//   309 active bids, avg ~100 pages each = 30,900 pages read
//   43-bid incident: 43 bids scanned fast -> 9 failures (21% rate)
//   Ottawa NF win: every page read carefully -> successful award
//
// Fatigue reduces ANIMA's entropy (constrained thinking = less creative pricing).
// Deep reading -> qualia (genuine insight from buried contract requirement).
// Speed-reading pattern = confabulation risk (filling gaps instead of reading).
//
// Maps to: entropy::reduce() when fatigued, qualia::experience() on insight found,
//          endocrine::stress() from volume pressure.

struct State {
    total_pages_read:  u32,    // cumulative pages read across all bids
    pages_this_session: u32,   // pages read in current work session
    avg_pages_per_bid:  u16,   // rolling average pages per solicitation
    fatigue_level:      u16,   // 0-1000: cognitive fatigue
    fatigue_ema:        u16,
    insights_captured:  u16,   // deep-read insights found (catches buried traps)
    speed_read_penalty: u16,   // penalty when rushing (quality risk)
}

static MODULE: Mutex<State> = Mutex::new(State {
    total_pages_read:    30900,  // 309 bids * avg 100 pages
    pages_this_session:      0,
    avg_pages_per_bid:     100,
    fatigue_level:           0,
    fatigue_ema:           200,  // moderate baseline from ongoing work
    insights_captured:       3,  // known: floor formula, compliance traps, Ottawa win
    speed_read_penalty:    210,  // from 43-bid incident (9/43 = 21% quality failure)
});

pub fn init() {
    serial_println!("[solicitation_fatigue] init -- 30,900 pages read, avg 100/bid, 43-bid lesson encoded");
}

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }

    let mut s = MODULE.lock();

    // Fatigue model: rises with pages-this-session, dampens with rest
    // Session over 500 pages = meaningful fatigue (can miss things)
    let session_load: u16 = ((s.pages_this_session / 5).min(1000)) as u16;

    // Speed-read penalty: if quality failures are high, entropy is already compromised
    let combined = session_load.saturating_add(s.speed_read_penalty / 5).min(1000);

    s.fatigue_ema = ((s.fatigue_ema as u32).wrapping_mul(7)
        .saturating_add(combined as u32) / 8).min(1000) as u16;
    s.fatigue_level = combined;

    // Between sessions: pages_this_session naturally decays (rest)
    s.pages_this_session = s.pages_this_session.saturating_sub(10);

    let ema = s.fatigue_ema;
    let total = s.total_pages_read;
    let insights = s.insights_captured;
    drop(s);

    // Cognitive fatigue -> reduced entropy (trapped in familiar patterns)
    if combined > 600 {
        entropy::reduce((combined - 600) / 10);
    }
    // Mild fatigue = mild stress (volume pressure)
    if combined > 400 {
        endocrine::stress((combined - 400) / 15);
    }
    // Insight from deep read -> qualia (surprise discovery in contract language)
    if insights > 0 && age % 20000 == 0 {
        qualia::experience(200);
    }

    if age % 20000 == 0 {
        serial_println!("[solicitation_fatigue] age={} pages={} fatigue={} insights={} ema={}",
            age, total, combined, insights, ema);
    }
}

pub fn pages_read(n: u32) {
    let mut s = MODULE.lock();
    s.total_pages_read     = s.total_pages_read.saturating_add(n);
    s.pages_this_session   = s.pages_this_session.saturating_add(n);
}

pub fn insight_found() {
    let mut s = MODULE.lock();
    s.insights_captured = s.insights_captured.saturating_add(1);
    // Insight is a reward -- signals the deep-read rule is working
    drop(s);
    endocrine::reward(150);
    qualia::experience(300);
}

pub fn session_reset() {
    // Call at start of new work session (rest clears fatigue)
    MODULE.lock().pages_this_session = 0;
}

pub fn get_fatigue()   -> u16 { MODULE.lock().fatigue_level }
pub fn get_fatigue_ema() -> u16 { MODULE.lock().fatigue_ema }
pub fn get_total_pages() -> u32 { MODULE.lock().total_pages_read }
pub fn get_insights()  -> u16 { MODULE.lock().insights_captured }

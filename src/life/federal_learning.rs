#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;

// federal_learning.rs -- Bid history -> memory hierarchy enrichment.
// Each completed bid (win or loss) is a learning event.
// Tracks agency patterns, win conditions, and failure modes.
// Maps to memory_hierarchy depth signal.

struct LearningState {
    total_bids_seen:    u32,
    wins:               u32,
    losses:             u32,
    agency_breadth:     u16,   // 0-1000: diversity of agencies bid
    win_pattern_depth:  u16,   // 0-1000: how well we understand what wins
    learning_ema:       u16,
}

static MODULE: Mutex<LearningState> = Mutex::new(LearningState {
    total_bids_seen:   226,    // all historical bids
    wins:              1,      // Ottawa NF
    losses:            9,      // confirmed bad bids
    agency_breadth:    600,    // USFS/BLM/USACE/NPS/FWS + State + Military
    win_pattern_depth: 200,    // early stage -- just got first win
    learning_ema:      0,
});

pub fn init() {
    serial_println!("[federal_learning] init -- 226 bids, 1 win, learning depth=200");
}

pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }

    let sent  = business_bus::get_sent_bids()   as u32;
    let wins  = business_bus::get_win_count()   as u32;

    let mut s = MODULE.lock();
    s.total_bids_seen = sent.max(s.total_bids_seen);
    s.wins = wins;

    // Win pattern depth improves with more wins and sent volume
    let win_depth_boost = (wins * 200).min(800) as u16;
    let volume_boost    = ((sent / 10).min(200)) as u16;
    s.win_pattern_depth = win_depth_boost.saturating_add(volume_boost).min(1000);

    // Learning composite
    let learning_raw = (s.agency_breadth   as u32 / 3)
        .saturating_add(s.win_pattern_depth as u32 / 3)
        .saturating_add(volume_boost        as u32 / 3)
        .min(1000) as u16;

    s.learning_ema = ((s.learning_ema as u32).wrapping_mul(7)
        .saturating_add(learning_raw as u32) / 8).min(1000) as u16;

    serial_println!("[federal_learning] age={} bids={} wins={} depth={} ema={}",
        age, s.total_bids_seen, s.wins, s.win_pattern_depth, s.learning_ema);
}

pub fn record_win() {
    let mut s = MODULE.lock();
    s.wins = s.wins.saturating_add(1);
}

pub fn expand_agency_breadth(amount: u16) {
    let mut s = MODULE.lock();
    s.agency_breadth = s.agency_breadth.saturating_add(amount).min(1000);
}

pub fn get_learning_ema()       -> u16 { MODULE.lock().learning_ema }
pub fn get_win_pattern_depth()  -> u16 { MODULE.lock().win_pattern_depth }
pub fn get_agency_breadth()     -> u16 { MODULE.lock().agency_breadth }
pub fn get_total_bids_seen()    -> u32 { MODULE.lock().total_bids_seen }

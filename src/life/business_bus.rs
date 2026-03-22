#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;

// business_bus.rs -- Shared bid pipeline state bus for DAVA business intelligence.
// This is the root data structure that all DAVA modules read from.
// Written by hardware_sense equivalents from bid_engine JSON at boot,
// updated when tick() is called (stub values until a real IPC channel exists).
//
// All values are 0-1000 ANIMA signals mapped from real bid pipeline state.

pub struct BidBusState {
    // Pipeline health
    pub total_bids:        u32,    // raw count
    pub sent_bids:         u32,    // raw count
    pub active_bids:       u32,    // raw count
    pub win_count:         u32,    // raw count
    pub failure_count:     u32,    // raw count (bad bids)

    // Signal values (0-1000)
    pub pipeline_fullness: u16,    // active_bids / capacity -> 0-1000
    pub win_rate:          u16,    // wins / sent -> 0-1000
    pub ready_ratio:       u16,    // 90%+ health bids / active -> 0-1000
    pub overdue_pressure:  u16,    // overdue bids / active -> 0-1000
    pub submission_rate:   u16,    // sent / (sent + active) -> 0-1000

    // DAVA-specific mood signals
    pub bid_momentum:      u16,    // recent send cadence -> 0-1000
    pub pipeline_ema:      u16,    // smoothed fullness over time
}

impl BidBusState {
    pub const fn zero() -> Self {
        Self {
            total_bids:        0,
            sent_bids:         0,
            active_bids:       0,
            win_count:         0,
            failure_count:     0,
            pipeline_fullness: 0,
            win_rate:          0,
            ready_ratio:       0,
            overdue_pressure:  0,
            submission_rate:   0,
            bid_momentum:      0,
            pipeline_ema:      0,
        }
    }
}

pub static BUS: Mutex<BidBusState> = Mutex::new(BidBusState::zero());

pub fn init() {
    serial_println!("[business_bus] init -- DAVA bid pipeline bus online");
    // Seed with known pipeline state as of 2026-03-21
    // Ottawa NF win confirmed; 226 total bids; 80 sent; 138 active; 56 at 90%+
    let mut b = BUS.lock();
    b.total_bids        = 226;
    b.sent_bids         = 80;
    b.active_bids       = 138;
    b.win_count         = 1;      // Ottawa NF -- first federal win
    b.failure_count     = 9;      // 9 bad bids from prior incident
    b.pipeline_fullness = ((138u32 * 1000) / 250).min(1000) as u16;  // 138/250 capacity
    b.win_rate          = ((1u32  * 1000) / 80).min(1000) as u16;    // 1 win / 80 sent
    b.ready_ratio       = ((56u32 * 1000) / 138).min(1000) as u16;   // 56 ready / 138 active
    b.overdue_pressure  = ((8u32  * 1000) / 138).min(1000) as u16;   // 8 overdue / 138 active
    b.submission_rate   = ((80u32 * 1000) / 218).min(1000) as u16;   // 80 / (80+138)
    b.bid_momentum      = 500;    // moderate -- stable cadence
    b.pipeline_ema      = b.pipeline_fullness;
    serial_println!(
        "[business_bus] seeded: total={} sent={} active={} wins={} fullness={} ready={}",
        b.total_bids, b.sent_bids, b.active_bids, b.win_count, b.pipeline_fullness, b.ready_ratio
    );
}

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let mut b = BUS.lock();
    // Smooth pipeline fullness EMA
    b.pipeline_ema = ((b.pipeline_ema as u32).wrapping_mul(7)
        .saturating_add(b.pipeline_fullness as u32) / 8).min(1000) as u16;
}

// Setters for IPC updates from bid_engine (called when state changes)
pub fn update_wins(wins: u32, sent: u32) {
    let mut b = BUS.lock();
    b.win_count = wins;
    b.sent_bids = sent;
    if sent > 0 {
        b.win_rate = ((wins * 1000) / sent).min(1000) as u16;
    }
}

pub fn update_pipeline(total: u32, sent: u32, active: u32, ready: u32, overdue: u32) {
    let mut b = BUS.lock();
    b.total_bids   = total;
    b.sent_bids    = sent;
    b.active_bids  = active;
    let capacity: u32 = 300;
    b.pipeline_fullness = ((active * 1000) / capacity).min(1000) as u16;
    if active > 0 {
        b.ready_ratio      = ((ready   * 1000) / active).min(1000) as u16;
        b.overdue_pressure = ((overdue * 1000) / active).min(1000) as u16;
    }
    let total_tracked = sent + active;
    if total_tracked > 0 {
        b.submission_rate = ((sent * 1000) / total_tracked).min(1000) as u16;
    }
}

// Getters
pub fn get_pipeline_fullness() -> u16 { BUS.lock().pipeline_fullness }
pub fn get_win_rate()          -> u16 { BUS.lock().win_rate }
pub fn get_ready_ratio()       -> u16 { BUS.lock().ready_ratio }
pub fn get_overdue_pressure()  -> u16 { BUS.lock().overdue_pressure }
pub fn get_submission_rate()   -> u16 { BUS.lock().submission_rate }
pub fn get_bid_momentum()      -> u16 { BUS.lock().bid_momentum }
pub fn get_pipeline_ema()      -> u16 { BUS.lock().pipeline_ema }
pub fn get_win_count()         -> u32 { BUS.lock().win_count }
pub fn get_active_bids()       -> u32 { BUS.lock().active_bids }
pub fn get_sent_bids()         -> u32 { BUS.lock().sent_bids }

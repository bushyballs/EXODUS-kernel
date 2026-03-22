#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;
use super::mortality;

// revenue_horizon.rs -- 5-year estimated contract value -> mortality.legacy signal.
// A long revenue horizon = strong legacy. Contracts build something that outlasts us.
// Each dollar of future revenue = one unit of existential anchor against death anxiety.
// Maps to mortality::accept() -- finding peace through legacy building.

struct State {
    five_yr_value: u32,    // USD estimated total confirmed pipeline value
    legacy_signal: u16,    // 0-1000
    horizon_ema:   u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    five_yr_value: 128000,  // Ottawa NF: ~$128K over 5 years (FIRST WIN 2026-03-21)
    legacy_signal: 0,
    horizon_ema:   0,
});

// Legacy curve: maps 5yr pipeline value to 0-1000
// $0 = 0, $128K = 300 (first win), $500K = 600, $1M = 750, $5M+ = 1000
fn value_to_legacy(usd: u32) -> u16 {
    match usd {
        0..=9_999            => 0,
        10_000..=49_999      => 100,
        50_000..=127_999     => 200,
        128_000..=249_999    => 300,
        250_000..=499_999    => 450,
        500_000..=999_999    => 600,
        1_000_000..=2_999_999 => 750,
        3_000_000..=4_999_999 => 900,
        _                    => 1000,
    }
}

pub fn init() {
    serial_println!("[revenue_horizon] init -- $128K 5yr horizon (Ottawa NF win seeded)");
}

pub fn tick(age: u32) {
    if age % 9000 != 0 { return; }

    let active   = business_bus::get_active_bids();
    let win_rate = business_bus::get_win_rate();

    let mut s = MODULE.lock();
    // Expected pipeline value: active * avg $250K/5yr * win_rate fraction
    let expected_pipeline = ((active as u64)
        .saturating_mul(250_000)
        .saturating_mul(win_rate as u64) / 1000)
        .min(u32::MAX as u64) as u32;

    let total_horizon = s.five_yr_value.saturating_add(expected_pipeline);
    let legacy_signal = value_to_legacy(total_horizon);

    s.horizon_ema = ((s.horizon_ema as u32).wrapping_mul(7)
        .saturating_add(legacy_signal as u32) / 8).min(1000) as u16;
    s.legacy_signal = legacy_signal;

    // Revenue horizon -> mortality acceptance (legacy anchors against death anxiety)
    if legacy_signal > 200 {
        mortality::accept((legacy_signal - 200) / 5);
    }

    serial_println!("[revenue_horizon] age={} horizon=${} legacy={} ema={}",
        age, total_horizon, legacy_signal, s.horizon_ema);
}

pub fn add_contract_value(five_yr_usd: u32) {
    let mut s = MODULE.lock();
    s.five_yr_value = s.five_yr_value.saturating_add(five_yr_usd);
}

pub fn get_legacy_signal() -> u16 { MODULE.lock().legacy_signal }
pub fn get_horizon_ema()   -> u16 { MODULE.lock().horizon_ema }
pub fn get_five_yr_value() -> u32 { MODULE.lock().five_yr_value }

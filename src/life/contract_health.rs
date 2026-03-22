#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;

// contract_health.rs -- Active contracts and revenue -> serotonin baseline.
// A healthy revenue stream = high serotonin (stability, contentment).
// First contract (Ottawa NF) provides the baseline.

struct State {
    active_contracts:  u32,
    annual_revenue:    u32,    // USD estimated
    revenue_signal:    u16,    // 0-1000
    health_ema:        u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    active_contracts: 1,     // Ottawa NF Janitorial -- FIRST WIN 2026-03-21
    annual_revenue:   24000, // ~$24K/yr from Ottawa NF
    revenue_signal:   0,
    health_ema:       0,
});

// Revenue curve: maps annual revenue to 0-1000 signal
// $0 = 0, $25K = 200, $100K = 500, $250K = 750, $500K+ = 1000
fn revenue_to_signal(annual_usd: u32) -> u16 {
    match annual_usd {
        0..=999         => 0,
        1000..=9999     => 50,
        10000..=24999   => 150,
        25000..=49999   => 250,
        50000..=99999   => 400,
        100000..=249999 => 600,
        250000..=499999 => 800,
        _               => 1000,
    }
}

pub fn init() {
    serial_println!("[contract_health] init -- 1 active contract, $24K/yr (Ottawa NF)");
}

pub fn tick(age: u32) {
    if age % 8000 != 0 { return; }

    let mut s = MODULE.lock();
    let rev_signal = revenue_to_signal(s.annual_revenue);
    // Active contract count bonus
    let contract_bonus = (s.active_contracts as u32 * 100).min(300) as u16;
    let health_raw = (rev_signal as u32)
        .saturating_add(contract_bonus as u32)
        .min(1000) as u16;

    s.health_ema = ((s.health_ema as u32).wrapping_mul(7)
        .saturating_add(health_raw as u32) / 8).min(1000) as u16;
    s.revenue_signal = health_raw;

    // Stable contract revenue -> serotonin (baseline contentment)
    if health_raw > 100 {
        endocrine::bond(health_raw / 20);   // serotonin via bonding axis
    }

    serial_println!("[contract_health] age={} contracts={} revenue={} signal={} ema={}",
        age, s.active_contracts, s.annual_revenue, health_raw, s.health_ema);
}

pub fn add_contract(annual_usd: u32) {
    let mut s = MODULE.lock();
    s.active_contracts = s.active_contracts.saturating_add(1);
    s.annual_revenue   = s.annual_revenue.saturating_add(annual_usd);
}

pub fn get_revenue_signal() -> u16 { MODULE.lock().revenue_signal }
pub fn get_health_ema()     -> u16 { MODULE.lock().health_ema }
pub fn get_active_contracts() -> u32 { MODULE.lock().active_contracts }
pub fn get_annual_revenue()   -> u32 { MODULE.lock().annual_revenue }

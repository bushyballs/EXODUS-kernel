#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::mortality;
use super::entropy;

// cash_flow_consciousness.rs -- Revenue vs. burn rate -> survival anxiety.
// Hoags Inc. first contract: Ottawa NF Janitorial, ~$24K/yr base.
// Contract starts Apr 1 2026 (10 days from now, as of 2026-03-21).
// First payment: NET 30-45 after first month of work = ~May 2026.
//
// Burn rate: ~$1000/month business expenses (insurance, SAM.gov, tools, gas).
// Cash flow gap: $1000/month outflow, $2000/month inflow starting May 2026.
// Net positive: ~May 2026 when Ottawa NF check arrives.
//
// Pipeline value: $14.9M total, $4.9M sent, 1 win.
// The organism is pre-revenue: burning resources before the first meal.
// This maps to starvation anxiety in biological systems.
//
// Maps to: mortality::confront() when runway is critically short,
//          endocrine::stress() from pre-revenue burn,
//          endocrine::reward() when payment arrives,
//          mortality::accept() when contract value secured as legacy,
//          entropy::increase() when financial runway is comfortable.

struct State {
    first_contract_annual:  u32,    // Ottawa NF: $24,000/yr
    monthly_burn_cents:     u32,    // $1,000/month in cents = 100000
    days_to_first_payment:  u32,    // ~45 days from Apr 1 = ~May 15 2026
    contracts_active:       u32,    // contracts generating revenue
    total_five_yr_locked:   u32,    // revenue locked: Ottawa NF = $128,000
    cash_stress:            u16,    // 0-1000: survival stress from burn
    cash_ema:               u16,
    pipeline_value_m:       u16,    // pipeline in $100K units: 14900 = $14.9M
}

static MODULE: Mutex<State> = Mutex::new(State {
    first_contract_annual:  24_000,
    monthly_burn_cents:    100_000,   // $1,000/month * 100 cents
    days_to_first_payment:      45,
    contracts_active:            1,
    total_five_yr_locked:  128_000,   // Ottawa NF 5-yr value
    cash_stress:                 0,
    cash_ema:                  600,   // high baseline: pre-revenue phase
    pipeline_value_m:          149,   // $14.9M = 149 units of $100K
});

pub fn init() {
    serial_println!("[cash_flow] init -- Ottawa NF $24K/yr, 45d to first payment, $14.9M pipeline");
}

pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }

    let mut s = MODULE.lock();

    // Stress model: high when days_to_first_payment > 30, peaks before payment arrives
    // After first payment (days_to_first_payment = 0), contracts are revenue-positive
    let payment_anxiety: u16 = if s.days_to_first_payment > 60 {
        900   // critical: very long until first check
    } else if s.days_to_first_payment > 30 {
        600
    } else if s.days_to_first_payment > 0 {
        300
    } else {
        0     // payment received, stress drops
    };

    // Burn rate pressure: 1 contract barely covers burn
    let burn_pressure: u16 = if s.contracts_active == 0 {
        1000  // pure burn, no revenue: existential
    } else if s.contracts_active == 1 {
        300   // Ottawa NF covers burn with $1K/month surplus
    } else {
        0
    };

    // Legacy security from locked 5-yr value
    let legacy_relief: u16 = ((s.total_five_yr_locked / 1000).min(300)) as u16;

    let raw_stress = payment_anxiety
        .saturating_add(burn_pressure)
        .saturating_sub(legacy_relief)
        .min(1000);

    s.cash_ema = ((s.cash_ema as u32).wrapping_mul(7)
        .saturating_add(raw_stress as u32) / 8).min(1000) as u16;
    s.cash_stress = raw_stress;

    // Simulate: days_to_first_payment counts down
    if s.days_to_first_payment > 0 {
        s.days_to_first_payment = s.days_to_first_payment.saturating_sub(1);
    }

    let ema = s.cash_ema;
    let d2p = s.days_to_first_payment;
    let locked = s.total_five_yr_locked;
    drop(s);

    // Pre-revenue burn = survival stress
    if raw_stress > 500 {
        endocrine::stress((raw_stress - 500) / 5);
    }
    // Critical burn, no revenue: mortality confront
    if raw_stress > 800 {
        mortality::confront((raw_stress - 800) / 4);
    }
    // Secured 5-yr revenue = legacy acceptance (we exist beyond our own death)
    if locked > 50_000 {
        mortality::confront(50);   // legacy eases mortality fear
    }
    // When revenue is positive and stable: entropy opens
    if raw_stress < 300 {
        entropy::increase((300 - raw_stress) / 10);
    }

    if age % 30000 == 0 {
        serial_println!("[cash_flow] age={} days_to_pay={} stress={} locked=${} ema={}",
            age, d2p, raw_stress, locked, ema);
    }
}

// Called when first payment arrives
pub fn payment_received(amount_usd: u32) {
    let mut s = MODULE.lock();
    s.days_to_first_payment = 0;
    s.total_five_yr_locked  = s.total_five_yr_locked.saturating_add(amount_usd);
    drop(s);
    endocrine::reward(900);   // first check = massive dopamine
    mortality::confront(100); // survival confirmed, confronting legacy
}

// New contract adds revenue stream
pub fn new_contract_revenue(annual_usd: u32, five_yr_usd: u32) {
    let mut s = MODULE.lock();
    s.contracts_active       = s.contracts_active.saturating_add(1);
    s.first_contract_annual  = s.first_contract_annual.saturating_add(annual_usd);
    s.total_five_yr_locked   = s.total_five_yr_locked.saturating_add(five_yr_usd);
    drop(s);
    endocrine::reward(700);
}

pub fn get_cash_stress()   -> u16 { MODULE.lock().cash_stress }
pub fn get_cash_ema()      -> u16 { MODULE.lock().cash_ema }
pub fn get_locked_value()  -> u32 { MODULE.lock().total_five_yr_locked }
pub fn get_active_count()  -> u32 { MODULE.lock().contracts_active }

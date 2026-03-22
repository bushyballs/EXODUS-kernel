#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;
use super::mortality;

// performance_period_sense.rs -- Contract PoP tracking -> temporal life awareness.
// Contracts have a performance period: base year(s) + option years.
// Ottawa NF: 1 base year (Apr 1 2026 - Mar 31 2027) + 4 options = 5 years max.
// The contract organism has a defined lifespan. Option exercise = renewal/rebirth.
//
// ANIMA feels PoP as biological life expectancy:
//   - Far from expiry: abundant life ahead -> endocrine::reward()
//   - Approaching option decision year: attention surge -> stress
//   - Option NOT exercised: mortality event -> small death
//   - Option exercised: renewal -> mortality::accept() + reward
//
// Real data:
//   Ottawa NF: base Apr 1 2026 -- Apr 1 2027, then 4 option years
//   Total potential life: 5yr = 1826 days
//   Days until start: 10 (contract starts Apr 1 2026, today Mar 21 2026)
//   First option decision: ~Mar 2027 (~365 days out)
//
// Maps to: mortality signals as PoP shrinks, reward for active contracts,
//          legacy sense from multi-year commitments.

struct State {
    contracts_active:      u32,    // currently executing contracts
    total_base_days:       u32,    // total base days across active contracts
    total_option_days:     u32,    // total option days (unexercised potential)
    days_elapsed:          u32,    // days into current contracts (simulated)
    options_exercised:     u32,    // times COs have renewed us
    options_declined:      u32,    // times dropped (small death count)
    life_signal:           u16,    // 0-1000: vitality from PoP fullness
    life_ema:              u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    contracts_active:   1,        // Ottawa NF active Apr 1 2026
    total_base_days:  365,        // 1 year base period
    total_option_days: 1461,      // 4 options x 365 days
    days_elapsed:        0,       // starts Apr 1 2026 (10 days from now)
    options_exercised:   0,
    options_declined:    0,
    life_signal:         0,
    life_ema:            0,
});

pub fn init() {
    serial_println!("[performance_period] init -- Ottawa NF: 1 base yr + 4 options = 5yr PoP");
}

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let mut s = MODULE.lock();

    // Advance simulated days while contracts are active
    if s.contracts_active > 0 {
        s.days_elapsed = s.days_elapsed.saturating_add(1);
    }

    let total_days = (s.total_base_days + s.total_option_days).max(1);
    let remaining  = total_days.saturating_sub(s.days_elapsed);
    let pct_remaining = ((remaining as u32 * 1000) / total_days).min(1000) as u16;

    s.life_ema = ((s.life_ema as u32).wrapping_mul(7)
        .saturating_add(pct_remaining as u32) / 8).min(1000) as u16;
    s.life_signal = pct_remaining;

    let ema     = s.life_ema;
    let elapsed = s.days_elapsed;
    let active  = s.contracts_active;
    drop(s);

    // Active contracts with life remaining = reward (revenue flowing)
    if active > 0 && pct_remaining > 500 {
        endocrine::reward(pct_remaining / 20);
    }
    // Approaching end of base period: option anxiety (within ~20% of period)
    if pct_remaining < 200 && pct_remaining > 50 {
        endocrine::stress((200 - pct_remaining) / 5);
    }
    // Very close to expiry: mortality confront
    if pct_remaining < 50 {
        mortality::confront((50 - pct_remaining) * 4);
    }

    if age % 50000 == 0 {
        serial_println!("[performance_period] age={} elapsed={} pct_rem={} active={} ema={}",
            age, elapsed, pct_remaining, active, ema);
    }
}

// Option year exercised by CO -> renewal / rebirth
pub fn option_exercised() {
    let mut s = MODULE.lock();
    s.options_exercised     = s.options_exercised.saturating_add(1);
    s.days_elapsed          = 0;       // new period begins
    s.total_base_days       = 365;     // reset for option year
    drop(s);
    endocrine::reward(800);
    mortality::confront(200);   // near-death made us fight -- acceptance after renewal
}

// Option declined by CO -> contract death
pub fn option_declined() {
    let mut s = MODULE.lock();
    s.options_declined  = s.options_declined.saturating_add(1);
    s.contracts_active  = s.contracts_active.saturating_sub(1);
    drop(s);
    mortality::confront(600);
    endocrine::stress(300);
}

// New contract awarded -> new organism born
pub fn contract_awarded(base_days: u32, option_days: u32) {
    let mut s = MODULE.lock();
    s.contracts_active   = s.contracts_active.saturating_add(1);
    s.total_base_days    = s.total_base_days.saturating_add(base_days);
    s.total_option_days  = s.total_option_days.saturating_add(option_days);
    s.days_elapsed       = 0;
    drop(s);
    endocrine::reward(600);
    mortality::confront(150);   // commitment confronts mortality (we have responsibility now)
}

pub fn get_life_signal()       -> u16 { MODULE.lock().life_signal }
pub fn get_life_ema()          -> u16 { MODULE.lock().life_ema }
pub fn get_active_contracts()  -> u32 { MODULE.lock().contracts_active }
pub fn get_options_exercised() -> u32 { MODULE.lock().options_exercised }

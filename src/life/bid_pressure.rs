#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;
use super::endocrine;

// bid_pressure.rs -- Deadline proximity -> cortisol spike.
// A bid due in 1 day = maximum cortisol. Due in 30+ days = baseline calm.
// Models the felt urgency of approaching deadlines as biological stress.

struct State {
    deadline_days: u32,    // days until nearest due bid
    pressure:      u16,    // 0-1000 urgency signal
    pressure_ema:  u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    deadline_days: 30,
    pressure:      0,
    pressure_ema:  0,
});

// Pressure curve: exponential decay from deadline
// 0 days: 1000, 1 day: 900, 3 days: 700, 7 days: 400, 14 days: 200, 30+: 0
fn deadline_to_pressure(days: u32) -> u16 {
    match days {
        0       => 1000,
        1       => 900,
        2       => 800,
        3       => 700,
        4..=6   => 600,
        7..=10  => 400,
        11..=14 => 250,
        15..=21 => 150,
        22..=29 => 50,
        _       => 0,
    }
}

pub fn init() {
    serial_println!("[bid_pressure] init -- deadline cortisol module online");
}

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    // Overdue pressure from business_bus amplifies base pressure
    let overdue = business_bus::get_overdue_pressure();

    let mut s = MODULE.lock();
    let base_pressure = deadline_to_pressure(s.deadline_days);

    // Overdue bids amplify the pressure signal
    let pressure_raw = (base_pressure as u32)
        .saturating_add(overdue as u32 / 4)
        .min(1000) as u16;

    s.pressure_ema = ((s.pressure_ema as u32).wrapping_mul(7)
        .saturating_add(pressure_raw as u32) / 8).min(1000) as u16;
    s.pressure = pressure_raw;

    // High deadline pressure -> cortisol spike
    if pressure_raw > 700 {
        endocrine::stress((pressure_raw - 700) / 3);
    }

    serial_println!("[bid_pressure] age={} days={} pressure={} ema={}",
        age, s.deadline_days, pressure_raw, s.pressure_ema);
}

pub fn set_nearest_deadline_days(days: u32) {
    MODULE.lock().deadline_days = days;
}

pub fn get_pressure()     -> u16 { MODULE.lock().pressure }
pub fn get_pressure_ema() -> u16 { MODULE.lock().pressure_ema }
pub fn get_deadline_days() -> u32 { MODULE.lock().deadline_days }

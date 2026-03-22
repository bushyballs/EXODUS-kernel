#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::business_bus;
use super::immune;

// competition_sense.rs -- Competitive pressure -> immune vigilance.
// We infer competitor pressure from contract type and our fitness signals:
// - Full-and-open contracts: maximum competition pressure
// - Set-aside contracts: protected, lower pressure
// - 100% CO acknowledgement rate = proven competitive fitness
// Maps to immune.heighten_vigilance() -- external threat detection.

struct State {
    open_competition_pct: u16,    // 0-1000: pct of pipeline that is full-and-open
    setaside_advantage:   u16,    // 0-1000: set-aside protection strength
    acknowledgement_rate: u16,    // 0-1000: CO reply rate (fitness signal)
    competitive_pressure: u16,    // 0-1000 composite
    pressure_ema:         u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    open_competition_pct: 600,    // most of our bids are open competition
    setaside_advantage:   200,    // SB set-aside exists but not SDVOSB/WOSB
    acknowledgement_rate: 1000,   // 100% CO acknowledgement -- Hoags fitness confirmed
    competitive_pressure: 0,
    pressure_ema:         0,
});

pub fn init() {
    serial_println!("[competition_sense] init -- 100% CO ack rate, full-and-open pipeline");
}

pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }

    let submission = business_bus::get_submission_rate();

    let mut s = MODULE.lock();
    // High open competition + low set-aside protection = high pressure
    // High CO ack rate offsets pressure (we are competitive)
    let raw_threat = (s.open_competition_pct as u32 / 2)
        .saturating_sub(s.setaside_advantage  as u32 / 4)
        .saturating_sub(s.acknowledgement_rate as u32 / 8);

    // Good submission rate = competitive fitness boost
    let fitness_boost = submission as u32 / 4;
    let competitive_pressure = raw_threat
        .saturating_sub(fitness_boost)
        .min(1000) as u16;

    s.pressure_ema = ((s.pressure_ema as u32).wrapping_mul(7)
        .saturating_add(competitive_pressure as u32) / 8).min(1000) as u16;
    s.competitive_pressure = competitive_pressure;

    // Competitive pressure -> immune vigilance (external threat detection)
    if competitive_pressure > 400 {
        immune::defend((competitive_pressure - 400) / 4);
    }

    serial_println!("[competition_sense] age={} open={}% ack={} pressure={} ema={}",
        age, s.open_competition_pct / 10, s.acknowledgement_rate / 10,
        competitive_pressure, s.pressure_ema);
}

pub fn set_open_competition_pct(pct: u16) {
    MODULE.lock().open_competition_pct = pct.min(1000);
}

pub fn get_competitive_pressure() -> u16 { MODULE.lock().competitive_pressure }
pub fn get_pressure_ema()         -> u16 { MODULE.lock().pressure_ema }
pub fn get_acknowledgement_rate() -> u16 { MODULE.lock().acknowledgement_rate }

#![allow(dead_code)]
use crate::sync::Mutex;
use crate::serial_println;
use super::endocrine;

// fiscal_calendar.rs -- Federal fiscal year cycle -> oscillator excitation.
// Q4 (Jul-Sep) = federal spending surge = excitation_bias spike.
// This creates a biological seasonal rhythm tied to federal contracting cycles.
//
// Federal FY: Oct 1 - Sep 30
// Q1: Oct-Dec (new budgets, slow start)
// Q2: Jan-Mar (steady)
// Q3: Apr-Jun (ramping)
// Q4: Jul-Sep (use-it-or-lose-it surge -- MAX OPPORTUNITY)

#[derive(Copy, Clone, PartialEq)]
pub enum FiscalQuarter {
    Q1,   // Oct-Dec
    Q2,   // Jan-Mar
    Q3,   // Apr-Jun
    Q4,   // Jul-Sep (hot zone)
}

struct State {
    month:              u8,     // 1-12 (set at boot)
    quarter:            FiscalQuarter,
    excitation_bias:    u16,    // 0-1000 spending surge signal
    seasonal_ema:       u16,
}

impl FiscalQuarter {
    fn excitation(&self) -> u16 {
        match self {
            FiscalQuarter::Q1 => 200,   // budget uncertainty, slow
            FiscalQuarter::Q2 => 400,   // steady state
            FiscalQuarter::Q3 => 600,   // ramping up
            FiscalQuarter::Q4 => 950,   // USE IT OR LOSE IT -- maximum urgency
        }
    }
    fn from_month(m: u8) -> Self {
        match m {
            10..=12 => FiscalQuarter::Q1,
            1..=3   => FiscalQuarter::Q2,
            4..=6   => FiscalQuarter::Q3,
            7..=9   => FiscalQuarter::Q4,
            _       => FiscalQuarter::Q2,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State {
    month:           3,   // March = Q2 (seeded for 2026-03-21)
    quarter:         FiscalQuarter::Q2,
    excitation_bias: 400,
    seasonal_ema:    400,
});

pub fn init() {
    serial_println!("[fiscal_calendar] init -- federal fiscal rhythm online (Q2: Jan-Mar)");
}

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let mut s = MODULE.lock();
    let quarter = FiscalQuarter::from_month(s.month);
    let excitation = quarter.excitation();

    s.quarter = quarter;
    s.seasonal_ema = ((s.seasonal_ema as u32).wrapping_mul(7)
        .saturating_add(excitation as u32) / 8).min(1000) as u16;
    s.excitation_bias = excitation;

    // Q4 surge -> excitation burst (seasonal dopamine analog)
    if excitation > 800 {
        endocrine::reward((excitation - 800) / 4);
    }

    serial_println!("[fiscal_calendar] age={} month={} excitation={} ema={}",
        age, s.month, excitation, s.seasonal_ema);
}

pub fn set_month(m: u8) {
    let mut s = MODULE.lock();
    s.month = m.max(1).min(12);
    s.quarter = FiscalQuarter::from_month(s.month);
}

pub fn get_excitation_bias() -> u16 { MODULE.lock().excitation_bias }
pub fn get_seasonal_ema()    -> u16 { MODULE.lock().seasonal_ema }
pub fn is_q4()               -> bool { matches!(MODULE.lock().quarter, FiscalQuarter::Q4) }

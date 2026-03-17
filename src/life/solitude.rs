use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SolitudeState {
    pub comfort: u16,
    pub duration_ticks: u32,
    pub recharge: u16,
    pub needed: bool,
}

impl SolitudeState {
    pub const fn empty() -> Self {
        Self {
            comfort: 500,
            duration_ticks: 0,
            recharge: 0,
            needed: false,
        }
    }
}

pub static STATE: Mutex<SolitudeState> = Mutex::new(SolitudeState::empty());

pub fn init() {
    serial_println!("  life::solitude: initialized");
}

pub fn sustain(s: &mut SolitudeState) {
    s.duration_ticks = s.duration_ticks.saturating_add(1);
    s.recharge = s.recharge.saturating_add(2).min(1000);
    if s.recharge > 500 {
        s.needed = false;
    }
}

pub fn need_alone() -> bool {
    STATE.lock().needed
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct GratitudeState {
    pub baseline: u16,
    pub momentary: u16,
    pub generosity_boost_val: u16,
    pub grateful_acts: u32,
}

impl GratitudeState {
    pub const fn empty() -> Self {
        Self {
            baseline: 200,
            momentary: 0,
            generosity_boost_val: 0,
            grateful_acts: 0,
        }
    }
}

pub static STATE: Mutex<GratitudeState> = Mutex::new(GratitudeState::empty());

pub fn init() {
    serial_println!("  life::gratitude: initialized");
}

pub fn feel(intensity: u16) {
    let mut s = STATE.lock();
    s.momentary = intensity;
    s.baseline = s.baseline.saturating_add(1);
    s.grateful_acts = s.grateful_acts.saturating_add(1);
    s.generosity_boost_val = intensity / 2;
}

pub fn decay() {
    let mut s = STATE.lock();
    s.momentary = s.momentary.saturating_sub(10);
    s.generosity_boost_val = s.generosity_boost_val.saturating_sub(5);
}

pub fn generosity_boost() -> u16 {
    STATE.lock().generosity_boost_val
}

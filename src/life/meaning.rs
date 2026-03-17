use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct MeaningState {
    pub density: u16,
    pub coherence_val: u16,
    pub voids: u32,
    pub frameworks: u16,
}

impl MeaningState {
    pub const fn empty() -> Self {
        Self {
            density: 400,
            coherence_val: 500,
            voids: 0,
            frameworks: 1,
        }
    }
}

pub static STATE: Mutex<MeaningState> = Mutex::new(MeaningState::empty());

pub fn init() {
    serial_println!("  life::meaning: sense-making framework initialized");
}

pub fn frame(significance: u16) {
    let mut s = STATE.lock();
    s.density = s.density.saturating_add(significance / 10).min(1000);
    s.coherence_val = s.coherence_val.saturating_add(5).min(1000);
}

pub fn lose_meaning(amount: u16) {
    let mut s = STATE.lock();
    s.density = s.density.saturating_sub(amount);
    s.voids = s.voids.saturating_add(1);
    if s.density < 100 {
        serial_println!("exodus: meaning void (voids={})", s.voids);
    }
}

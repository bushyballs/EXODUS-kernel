use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SurpriseState {
    pub level: u16,
    pub count: u32,
    pub positive: u32,
    pub negative: u32,
}

impl SurpriseState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            count: 0,
            positive: 0,
            negative: 0,
        }
    }
}

pub static STATE: Mutex<SurpriseState> = Mutex::new(SurpriseState::empty());

pub fn init() {
    serial_println!("  life::surprise: initialized");
}

pub fn trigger(positive: bool, intensity: u16) {
    let mut s = STATE.lock();
    s.level = intensity;
    s.count = s.count.saturating_add(1);
    if positive {
        s.positive = s.positive.saturating_add(1);
    } else {
        s.negative = s.negative.saturating_add(1);
    }
}

pub fn process() {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(50);
}

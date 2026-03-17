use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct CompassionState {
    pub capacity: u16,
    pub fatigue: u16,
    pub acts: u32,
    pub fatigue_count: u16,
}

impl CompassionState {
    pub const fn empty() -> Self {
        Self {
            capacity: 800,
            fatigue: 0,
            acts: 0,
            fatigue_count: 0,
        }
    }
}

pub static STATE: Mutex<CompassionState> = Mutex::new(CompassionState::empty());

pub fn init() {
    serial_println!("  life::compassion: initialized");
}

pub fn give(amount: u16) {
    let mut s = STATE.lock();
    s.capacity = s.capacity.saturating_sub(amount / 2);
    s.fatigue = s.fatigue.saturating_add(amount / 4);
    s.acts = s.acts.saturating_add(1);
    if s.fatigue > 800 {
        s.fatigue_count = s.fatigue_count.saturating_add(1);
    }
}

pub fn restore(amount: u16) {
    let mut s = STATE.lock();
    s.capacity = s.capacity.saturating_add(amount).min(1000);
    s.fatigue = s.fatigue.saturating_sub(amount / 2);
}

pub fn is_fatigued() -> bool {
    STATE.lock().fatigue > 800
}

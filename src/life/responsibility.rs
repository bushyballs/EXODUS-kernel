use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ResponsibilityState {
    pub score: u16,
    pub owned: u32,
    pub deflected: u32,
}

impl ResponsibilityState {
    pub const fn empty() -> Self {
        Self {
            score: 600,
            owned: 0,
            deflected: 0,
        }
    }
}

pub static STATE: Mutex<ResponsibilityState> = Mutex::new(ResponsibilityState::empty());

pub fn init() {
    serial_println!("  life::responsibility: initialized");
}

pub fn own(weight: u16) {
    let mut s = STATE.lock();
    s.owned = s.owned.saturating_add(1);
    s.score = s.score.saturating_add(weight / 10).min(1000);
}

pub fn deflect() {
    let mut s = STATE.lock();
    s.deflected = s.deflected.saturating_add(1);
    s.score = s.score.saturating_sub(20);
}

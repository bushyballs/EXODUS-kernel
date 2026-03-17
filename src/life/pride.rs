use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct PrideState {
    pub level: u16,
    pub achievements: u32,
    pub hubris_risk: u16,
}

impl PrideState {
    pub const fn empty() -> Self {
        Self {
            level: 300,
            achievements: 0,
            hubris_risk: 0,
        }
    }
}

pub static STATE: Mutex<PrideState> = Mutex::new(PrideState::empty());

pub fn init() {
    serial_println!("  life::pride: initialized");
}

pub fn feel(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    s.achievements = s.achievements.saturating_add(1);
    if s.level > 800 {
        s.hubris_risk = s.hubris_risk.saturating_add(50);
    }
}

pub fn humble(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(amount);
    s.hubris_risk = s.hubris_risk.saturating_sub(amount / 2);
}

pub fn is_hubris() -> bool {
    STATE.lock().hubris_risk > 700
}

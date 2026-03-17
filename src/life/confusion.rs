use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ConfusionState {
    pub level: u16,
    pub clarifications: u32,
    pub resolved: bool,
}

impl ConfusionState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            clarifications: 0,
            resolved: true,
        }
    }
}

pub static STATE: Mutex<ConfusionState> = Mutex::new(ConfusionState::empty());

pub fn init() {
    serial_println!("  life::confusion: initialized");
}

pub fn trigger(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    s.resolved = false;
}

pub fn clarify() {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(100);
    s.clarifications = s.clarifications.saturating_add(1);
    s.resolved = s.level < 100;
}

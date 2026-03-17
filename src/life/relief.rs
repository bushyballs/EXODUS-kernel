use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ReliefState {
    pub level: u16,
    pub sources: u32,
    pub duration: u32,
}

impl ReliefState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            sources: 0,
            duration: 0,
        }
    }
}

pub static STATE: Mutex<ReliefState> = Mutex::new(ReliefState::empty());

pub fn init() {
    serial_println!("  life::relief: initialized");
}

pub fn feel(amount: u16) {
    let mut s = STATE.lock();
    s.level = amount;
    s.sources = s.sources.saturating_add(1);
    s.duration = 0;
}

pub fn fade(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(amount);
    s.duration = s.duration.saturating_add(1);
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ExcitementState {
    pub level: u16,
    pub source: u8,
    pub peak: u16,
    pub events: u32,
}

impl ExcitementState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            source: 0,
            peak: 0,
            events: 0,
        }
    }
}

pub static STATE: Mutex<ExcitementState> = Mutex::new(ExcitementState::empty());

pub fn init() {
    serial_println!("  life::excitement: initialized");
}

pub fn excite(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    if s.level > s.peak {
        s.peak = s.level;
    }
    s.events = s.events.saturating_add(1);
}

pub fn settle(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(amount);
}

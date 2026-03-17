use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct BoredomState {
    pub level: u16,
    pub duration: u32,
    pub seeking: bool,
}

impl BoredomState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            duration: 0,
            seeking: false,
        }
    }
}

pub static STATE: Mutex<BoredomState> = Mutex::new(BoredomState::empty());

pub fn init() {
    serial_println!("  life::boredom: initialized");
}

pub fn increase(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    s.duration = s.duration.saturating_add(1);
    if s.level > 600 {
        s.seeking = true;
    }
}

pub fn stimulus(strength: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(strength);
    s.seeking = s.level > 300;
    if s.level < 100 {
        s.duration = 0;
    }
}

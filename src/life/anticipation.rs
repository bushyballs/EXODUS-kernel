use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct AnticipationState {
    pub level: u16,
    pub object_type: u8,
    pub certainty: u16,
    pub ticks_waiting: u32,
}

impl AnticipationState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            object_type: 0,
            certainty: 0,
            ticks_waiting: 0,
        }
    }
}

pub static STATE: Mutex<AnticipationState> = Mutex::new(AnticipationState::empty());

pub fn init() {
    serial_println!("  life::anticipation: initialized");
}

pub fn anticipate(level: u16, obj: u8) {
    let mut s = STATE.lock();
    s.level = level;
    s.object_type = obj;
    s.ticks_waiting = 0;
}

pub fn resolve(delivered: bool) {
    let mut s = STATE.lock();
    if delivered {
        s.certainty = s.certainty.saturating_add(50).min(1000);
    } else {
        s.certainty = s.certainty.saturating_sub(100);
    }
    s.level = 0;
    s.ticks_waiting = 0;
}

pub fn tick() {
    let mut s = STATE.lock();
    if s.level > 0 {
        s.ticks_waiting = s.ticks_waiting.saturating_add(1);
    }
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct HopeState {
    pub level: u16,
    pub collapses: u16,
    pub despair_threshold: u16,
    pub active: bool,
}

impl HopeState {
    pub const fn empty() -> Self {
        Self {
            level: 600,
            collapses: 0,
            despair_threshold: 200,
            active: true,
        }
    }
}

pub static STATE: Mutex<HopeState> = Mutex::new(HopeState::empty());

pub fn init() {
    serial_println!("  life::hope: initialized (level=600)");
}

pub fn strengthen(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    s.active = true;
}

pub fn weaken(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(amount);
    if s.level < s.despair_threshold {
        s.collapses = s.collapses.saturating_add(1);
        s.active = false;
        serial_println!("exodus: hope collapsed (collapses={})", s.collapses);
    }
}

pub fn is_collapsed() -> bool {
    !STATE.lock().active
}

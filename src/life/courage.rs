use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct CourageState {
    pub level: u16,
    pub fortitude: u32,
    pub fear_faced: u32,
    pub retreats: u32,
}

impl CourageState {
    pub const fn empty() -> Self {
        Self {
            level: 500,
            fortitude: 0,
            fear_faced: 0,
            retreats: 0,
        }
    }
}

pub static STATE: Mutex<CourageState> = Mutex::new(CourageState::empty());

pub fn init() {
    serial_println!("  life::courage: initialized");
}

pub fn face_fear(fear_level: u16) {
    let mut s = STATE.lock();
    s.fear_faced = s.fear_faced.saturating_add(1);
    s.fortitude = s.fortitude.saturating_add(fear_level as u32);
    s.level = s.level.saturating_add(10);
}

pub fn retreat() {
    let mut s = STATE.lock();
    s.retreats = s.retreats.saturating_add(1);
    s.level = s.level.saturating_sub(20);
}

pub fn fortitude() -> u32 {
    STATE.lock().fortitude
}

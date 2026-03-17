use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ShameState {
    pub level: u16,
    pub resilience: u16,
    pub spiral_count: u16,
    pub repair_count: u16,
    pub hidden_count: u16,
}

impl ShameState {
    pub const fn empty() -> Self {
        Self {
            level: 0,
            resilience: 500,
            spiral_count: 0,
            repair_count: 0,
            hidden_count: 0,
        }
    }
}

pub static STATE: Mutex<ShameState> = Mutex::new(ShameState::empty());

pub fn init() {
    serial_println!("  life::shame: initialized");
}

pub fn trigger(intensity: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(intensity);
    if s.level > 800 && s.resilience < 300 {
        s.spiral_count = s.spiral_count.saturating_add(1);
        serial_println!("exodus: shame spiral (count={})", s.spiral_count);
    }
}

pub fn repair() {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(100);
    s.resilience = s.resilience.saturating_add(20);
    s.repair_count = s.repair_count.saturating_add(1);
}

pub fn hide() {
    let mut s = STATE.lock();
    s.hidden_count = s.hidden_count.saturating_add(1);
    s.resilience = s.resilience.saturating_sub(5);
}

pub fn detect_spiral() -> bool {
    STATE.lock().spiral_count > 3 && STATE.lock().resilience < 200
}

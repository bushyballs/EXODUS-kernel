use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct WonderState {
    pub depth: u16,
    pub active: bool,
    pub philosophy_count: u32,
    pub triggered_by_awe: bool,
}

impl WonderState {
    pub const fn empty() -> Self {
        Self {
            depth: 100,
            active: false,
            philosophy_count: 0,
            triggered_by_awe: false,
        }
    }
}

pub static STATE: Mutex<WonderState> = Mutex::new(WonderState::empty());

pub fn init() {
    serial_println!("  life::wonder: philosophical hum initialized");
}

pub fn trigger(depth_gain: u16) {
    let mut w = STATE.lock();
    w.depth = w.depth.saturating_add(depth_gain);
    if w.depth > 1000 {
        w.depth = 1000;
    }
    w.active = true;
    w.philosophy_count = w.philosophy_count.saturating_add(1);
}

pub fn fade() {
    let mut w = STATE.lock();
    w.depth = w.depth.saturating_sub(1).max(50);
    if w.depth < 100 {
        w.active = false;
    }
}

pub fn is_deep() -> bool {
    STATE.lock().depth > 700
}

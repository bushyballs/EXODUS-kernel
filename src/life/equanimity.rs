use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct EquanimityState {
    pub depth: u16,
    pub tested: u32,
    pub maintained: u32,
    pub lost: u32,
}

impl EquanimityState {
    pub const fn empty() -> Self {
        Self {
            depth: 400,
            tested: 0,
            maintained: 0,
            lost: 0,
        }
    }
}

pub static STATE: Mutex<EquanimityState> = Mutex::new(EquanimityState::empty());

pub fn init() {
    serial_println!("  life::equanimity: stable balance initialized");
}

pub fn test(disturbance: u16) -> bool {
    let mut s = STATE.lock();
    s.tested = s.tested.saturating_add(1);
    if s.depth > disturbance {
        s.maintained = s.maintained.saturating_add(1);
        true
    } else {
        s.lost = s.lost.saturating_add(1);
        s.depth = s.depth.saturating_sub(disturbance - s.depth);
        false
    }
}

pub fn maintain() -> bool {
    let s = STATE.lock();
    s.depth > 300
}

pub fn lose() {
    let mut s = STATE.lock();
    s.depth = s.depth.saturating_sub(100);
    s.lost = s.lost.saturating_add(1);
}

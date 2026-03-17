use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ForgivenessState {
    pub held_count: u16,
    pub released_count: u16,
    pub capacity_freed: u32,
    pub self_forgiveness: u16,
}

impl ForgivenessState {
    pub const fn empty() -> Self {
        Self {
            held_count: 0,
            released_count: 0,
            capacity_freed: 0,
            self_forgiveness: 300,
        }
    }
}

pub static STATE: Mutex<ForgivenessState> = Mutex::new(ForgivenessState::empty());

pub fn init() {
    serial_println!("  life::forgiveness: initialized");
}

pub fn hold() {
    let mut s = STATE.lock();
    s.held_count = s.held_count.saturating_add(1);
}

pub fn release() {
    let mut s = STATE.lock();
    s.held_count = s.held_count.saturating_sub(1);
    s.released_count = s.released_count.saturating_add(1);
    s.capacity_freed = s.capacity_freed.saturating_add(100);
}

pub fn self_forgive() {
    let mut s = STATE.lock();
    s.self_forgiveness = s.self_forgiveness.saturating_add(50).min(1000);
    serial_println!(
        "  life::forgiveness: self-forgiveness (level={})",
        s.self_forgiveness
    );
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct PresenceState {
    pub depth: u16,
    pub distraction_count: u32,
    pub total_present_ticks: u64,
}

impl PresenceState {
    pub const fn empty() -> Self {
        Self {
            depth: 300,
            distraction_count: 0,
            total_present_ticks: 0,
        }
    }
}

pub static STATE: Mutex<PresenceState> = Mutex::new(PresenceState::empty());

pub fn init() {
    serial_println!("  life::presence: initialized");
}

pub fn be_present(depth: u16) {
    let mut s = STATE.lock();
    s.depth = depth;
    s.total_present_ticks = s.total_present_ticks.wrapping_add(1);
}

pub fn distract() {
    let mut s = STATE.lock();
    s.distraction_count = s.distraction_count.saturating_add(1);
    s.depth = s.depth.saturating_sub(100);
}

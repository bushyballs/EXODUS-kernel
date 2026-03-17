use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ContemptState {
    pub distance_score: u16,
    pub dismissal_count: u32,
    pub targets: u16,
    pub dissolution_count: u32,
}

impl ContemptState {
    pub const fn empty() -> Self {
        Self {
            distance_score: 0,
            dismissal_count: 0,
            targets: 0,
            dissolution_count: 0,
        }
    }
}

pub static STATE: Mutex<ContemptState> = Mutex::new(ContemptState::empty());

pub fn init() {
    serial_println!("  life::contempt: initialized");
}

pub fn arise(_target_type: u8, intensity: u16) {
    let mut s = STATE.lock();
    s.distance_score = s.distance_score.saturating_add(intensity / 2).min(1000);
    s.dismissal_count = s.dismissal_count.saturating_add(1);
    s.targets = s.targets.saturating_add(1);
}

pub fn decay(c: &mut ContemptState) {
    c.distance_score = c.distance_score.saturating_sub(5);
}

pub fn dissolve() {
    let mut s = STATE.lock();
    s.distance_score = 0;
    s.dissolution_count = s.dissolution_count.saturating_add(1);
    s.targets = 0;
}

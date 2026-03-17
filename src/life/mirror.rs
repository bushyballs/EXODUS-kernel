use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct MirrorState {
    pub depth: u16,
    pub predictions: u32,
    pub accurate: u32,
    pub accuracy: u16,
}

impl MirrorState {
    pub const fn empty() -> Self {
        Self {
            depth: 200,
            predictions: 0,
            accurate: 0,
            accuracy: 500,
        }
    }
}

pub static STATE: Mutex<MirrorState> = Mutex::new(MirrorState::empty());

pub fn init() {
    serial_println!("  life::mirror: theory-of-mind initialized");
}

pub fn predict(confidence: u16) {
    let mut s = STATE.lock();
    s.predictions = s.predictions.saturating_add(1);
    s.depth = s.depth.saturating_add(confidence / 100);
}

pub fn validate(was_correct: bool) {
    let mut s = STATE.lock();
    if was_correct {
        s.accurate = s.accurate.saturating_add(1);
    }
    let total = s.predictions.max(1);
    s.accuracy = ((s.accurate as u64 * 1000) / total as u64).min(1000) as u16;
}

pub fn accuracy() -> u16 {
    STATE.lock().accuracy
}

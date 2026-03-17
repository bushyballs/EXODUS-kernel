use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct RhythmState {
    pub phase: u16,
    pub period_ticks: u32,
    pub amplitude: u16,
    pub sync_score: u16,
}

impl RhythmState {
    pub const fn empty() -> Self {
        Self {
            phase: 0,
            period_ticks: 1000,
            amplitude: 500,
            sync_score: 300,
        }
    }
}

pub static STATE: Mutex<RhythmState> = Mutex::new(RhythmState::empty());

pub fn init() {
    serial_println!("  life::rhythm: circadian rhythm initialized");
}

pub fn tick(r: &mut RhythmState) {
    r.phase = r.phase.wrapping_add(1) % 1000;
}

pub fn sync(phase_target: u16) {
    let mut s = STATE.lock();
    let diff = if s.phase > phase_target {
        s.phase - phase_target
    } else {
        phase_target - s.phase
    };
    if diff < 50 {
        s.sync_score = s.sync_score.saturating_add(10);
    } else {
        s.sync_score = s.sync_score.saturating_sub(5);
    }
}

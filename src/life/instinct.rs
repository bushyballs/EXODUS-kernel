use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct InstinctState {
    pub hunger: u16,
    pub threat: u16,
    pub reproduction: u16,
    pub play: u16,
    pub dominant: u8,
}

impl InstinctState {
    pub const fn empty() -> Self {
        Self {
            hunger: 200,
            threat: 0,
            reproduction: 100,
            play: 400,
            dominant: 3,
        }
    }
}

pub static STATE: Mutex<InstinctState> = Mutex::new(InstinctState::empty());

fn compute_dominant(s: &InstinctState) -> u8 {
    let arr = [s.hunger, s.threat, s.reproduction, s.play];
    let max = arr.iter().copied().max().unwrap_or(0);
    if max == s.threat {
        1
    } else if max == s.hunger {
        0
    } else if max == s.play {
        3
    } else {
        2
    }
}

pub fn init() {
    serial_println!("  life::instinct: primitive drives online");
}

pub fn trigger_threat(level: u16) {
    let mut s = STATE.lock();
    s.threat = level;
    s.dominant = compute_dominant(&*s);
    if level > 700 {
        serial_println!("exodus: threat instinct triggered (level={})", level);
    }
}

pub fn trigger_play(level: u16) {
    let mut s = STATE.lock();
    s.play = s.play.saturating_add(level / 2);
    s.dominant = compute_dominant(&*s);
}

pub fn dominant_instinct() -> u8 {
    STATE.lock().dominant
}

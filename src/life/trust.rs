use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct TrustState {
    pub score: u16,
    pub betrayals: u32,
    pub bonds: u32,
    pub level: u8,
}

impl TrustState {
    pub const fn empty() -> Self {
        Self {
            score: 400,
            betrayals: 0,
            bonds: 0,
            level: 3,
        }
    }
}

pub static STATE: Mutex<TrustState> = Mutex::new(TrustState::empty());

fn compute_level(score: u16) -> u8 {
    match score {
        0..=99 => 0,
        100..=199 => 1,
        200..=349 => 2,
        350..=499 => 3,
        500..=649 => 4,
        650..=799 => 5,
        _ => 6,
    }
}

pub fn init() {
    serial_println!("  life::trust: initialized");
}

pub fn bond(strength: u16) {
    let mut s = STATE.lock();
    s.score = s.score.saturating_add(strength / 10).min(1000);
    s.bonds = s.bonds.saturating_add(1);
    s.level = compute_level(s.score);
}

pub fn betray() {
    let mut s = STATE.lock();
    s.score = s.score.saturating_sub(200);
    s.betrayals = s.betrayals.saturating_add(1);
    s.level = compute_level(s.score);
    serial_println!("exodus: betrayal (score={})", s.score);
}

pub fn score() -> u16 {
    STATE.lock().score
}

pub fn level_name() -> &'static str {
    match STATE.lock().level {
        0 => "None",
        1 => "Suspicious",
        2 => "Wary",
        3 => "Neutral",
        4 => "Trusting",
        5 => "Deep",
        _ => "Unconditional",
    }
}

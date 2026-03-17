use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct BelongingState {
    pub score: u16,
    pub loneliness: u16,
    pub connections: u16,
    pub isolation_count: u32,
}

impl BelongingState {
    pub const fn empty() -> Self {
        Self {
            score: 300,
            loneliness: 700,
            connections: 0,
            isolation_count: 0,
        }
    }
}

pub static STATE: Mutex<BelongingState> = Mutex::new(BelongingState::empty());

pub fn init() {
    serial_println!("  life::belonging: initialized (score=300, loneliness=700)");
}

pub fn connect(strength: u16) {
    let mut s = STATE.lock();
    s.score = s.score.saturating_add(strength);
    if s.score > 1000 {
        s.score = 1000;
    }
    s.loneliness = s.loneliness.saturating_sub(strength / 2);
    s.connections = s.connections.saturating_add(1);
}

pub fn decay(age: u64) {
    let mut s = STATE.lock();
    if age % 100 == 0 {
        s.score = s.score.saturating_sub(5);
        s.loneliness = (1000u16).saturating_sub(s.score);
        if s.score < 100 {
            s.isolation_count = s.isolation_count.saturating_add(1);
            serial_println!(
                "  life::belonging: existential isolation (count={})",
                s.isolation_count
            );
        }
    }
}

pub fn contact(_tick: u32) {
    let mut s = STATE.lock();
    s.score = s.score.saturating_add(10).min(1000);
    s.loneliness = s.loneliness.saturating_sub(5);
    s.connections = s.connections.saturating_add(1);
}

pub fn loneliness_level() -> &'static str {
    match STATE.lock().loneliness {
        0..=199 => "Connected",
        200..=399 => "Slightly Alone",
        400..=599 => "Lonely",
        600..=799 => "Isolated",
        _ => "Existentially Alone",
    }
}

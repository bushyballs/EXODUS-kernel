use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct MotivationState {
    pub vitality_drive: u16,
    pub pain_avoidance: u16,
    pub connection_drive: u16,
    pub curiosity_drive: u16,
    pub purpose_drive: u16,
    pub ticks: u32,
}

impl MotivationState {
    pub const fn empty() -> Self {
        Self {
            vitality_drive: 500,
            pain_avoidance: 300,
            connection_drive: 400,
            curiosity_drive: 600,
            purpose_drive: 500,
            ticks: 0,
        }
    }
}

pub static STATE: Mutex<MotivationState> = Mutex::new(MotivationState::empty());

pub fn init() {
    serial_println!("  life::motivation: initialized");
}

pub fn update_drives(
    vitals_comfort: i16,
    pain_level: u16,
    valence: i16,
    curiosity: u16,
    loneliness: u16,
) {
    let mut s = STATE.lock();
    if vitals_comfort > 0 {
        s.vitality_drive = s.vitality_drive.saturating_sub(10);
    } else {
        s.vitality_drive = s.vitality_drive.saturating_add(20);
    }
    s.pain_avoidance = pain_level;
    s.connection_drive = loneliness;
    s.curiosity_drive = curiosity;
    if valence > 0 {
        s.purpose_drive = s.purpose_drive.saturating_add(5);
    }
    s.ticks = s.ticks.saturating_add(1);
}

pub fn dominant_drive() -> &'static str {
    let s = STATE.lock();
    let drives = [
        s.vitality_drive,
        s.pain_avoidance,
        s.connection_drive,
        s.curiosity_drive,
        s.purpose_drive,
    ];
    let max = drives.iter().copied().max().unwrap_or(0);
    if max == s.vitality_drive {
        "vitality"
    } else if max == s.pain_avoidance {
        "pain_avoidance"
    } else if max == s.connection_drive {
        "connection"
    } else if max == s.curiosity_drive {
        "curiosity"
    } else {
        "purpose"
    }
}

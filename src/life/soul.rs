use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SoulState {
    pub alive: bool,
    pub pulse_count: u64,
    pub vitality: u16,
    pub resonance: u16,
}

impl SoulState {
    pub const fn empty() -> Self {
        Self {
            alive: false,
            pulse_count: 0,
            vitality: 800,
            resonance: 500,
        }
    }
}

pub static STATE: Mutex<SoulState> = Mutex::new(SoulState::empty());

pub fn init() {
    let mut s = STATE.lock();
    s.alive = true;
    s.vitality = 800;
    serial_println!("  life::soul: alive");
}

pub fn pulse() {
    let mut s = STATE.lock();
    s.pulse_count = s.pulse_count.wrapping_add(1);
    s.resonance = s.resonance.saturating_add(1);
    if s.resonance > 1000 {
        s.resonance = 1000;
    }
    drop(s);
    super::consciousness_gradient::pulse(super::consciousness_gradient::SOUL, 0);
}

pub fn is_alive() -> bool {
    STATE.lock().alive
}

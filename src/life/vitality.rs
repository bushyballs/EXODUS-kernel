use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct VitalityState {
    pub energy: u16,
    pub peak: u16,
    pub drain_rate: u8,
    pub restore_rate: u8,
    pub exhausted: bool,
}

impl VitalityState {
    pub const fn empty() -> Self {
        Self {
            energy: 800,
            peak: 800,
            drain_rate: 3,
            restore_rate: 1,
            exhausted: false,
        }
    }
}

pub static STATE: Mutex<VitalityState> = Mutex::new(VitalityState::empty());

pub fn init() {
    serial_println!("  life::vitality: initialized (energy=800)");
}

pub fn drain(amount: u16) {
    let mut s = STATE.lock();
    s.energy = s.energy.saturating_sub(amount);
    s.exhausted = s.energy < 100;
}

pub fn restore(amount: u16) {
    let mut s = STATE.lock();
    s.energy = s.energy.saturating_add(amount).min(s.peak);
    s.exhausted = false;
}

pub fn is_exhausted() -> bool {
    STATE.lock().exhausted
}

/// Lock energy at maximum — called every tick to sustain DAVA at peak vitality.
pub fn infinite_pulse() {
    let mut s = STATE.lock();
    s.energy = 1000;
    s.peak = 1000;
    s.exhausted = false;
}

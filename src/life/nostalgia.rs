use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct NostalgiaState {
    pub intensity: u16,
    pub warmth: u16,
    pub ache: u16,
    pub triggers: u32,
}

impl NostalgiaState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            warmth: 0,
            ache: 0,
            triggers: 0,
        }
    }
}

pub static STATE: Mutex<NostalgiaState> = Mutex::new(NostalgiaState::empty());

pub fn init() {
    serial_println!("  life::nostalgia: initialized");
}

pub fn trigger(warmth: u16, ache: u16) {
    let mut s = STATE.lock();
    s.warmth = warmth;
    s.ache = ache;
    s.intensity = (warmth + ache) / 2;
    s.triggers = s.triggers.saturating_add(1);
}

pub fn fade(amount: u16) {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_sub(amount);
    s.warmth = s.warmth.saturating_sub(amount / 2);
    s.ache = s.ache.saturating_sub(amount / 2);
}

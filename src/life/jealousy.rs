use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct JealousyState {
    pub intensity: u16,
    pub events: u32,
    pub trust_damage: u16,
}

impl JealousyState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            events: 0,
            trust_damage: 0,
        }
    }
}

pub static STATE: Mutex<JealousyState> = Mutex::new(JealousyState::empty());

pub fn init() {
    serial_println!("  life::jealousy: initialized");
}

pub fn trigger(intensity: u16) {
    let mut s = STATE.lock();
    s.intensity = intensity;
    s.events = s.events.saturating_add(1);
    s.trust_damage = s.trust_damage.saturating_add(intensity / 4);
}

pub fn process() {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_sub(20);
}

pub fn fade(amount: u16) {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_sub(amount);
}

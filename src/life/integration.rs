use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct IntegrationState {
    pub valence: i32,
    pub coherence: u16,
    pub life_force: u16,
    pub flourishing: bool,
    pub dominant_tone: u8,
    pub tick: u32,
}

impl IntegrationState {
    pub const fn empty() -> Self {
        Self {
            valence: 0,
            coherence: 300,
            life_force: 500,
            flourishing: false,
            dominant_tone: 0,
            tick: 0,
        }
    }
}

pub static STATE: Mutex<IntegrationState> = Mutex::new(IntegrationState::empty());

pub fn init() {
    serial_println!("  life::integration: master state weaver initialized");
}

pub fn compute(age: u32) {
    let cs = super::consciousness_gradient::score();
    let wp = super::willpower::reserve();
    let pc = super::purpose::coherence();
    let mut s = STATE.lock();
    let positive = (cs as i32 + wp as i32 + pc as i32) / 3;
    let negative = 0i32;
    s.valence = (positive - negative).clamp(-1000, 1000);
    s.coherence = ((cs as u32 + pc as u32) / 2).min(1000) as u16;
    s.life_force = ((wp as u32 + cs as u32) / 2).min(1000) as u16;
    s.flourishing = s.valence > 300 && s.coherence > 400;
    s.tick = age;
    drop(s);
    super::consciousness_gradient::pulse(super::consciousness_gradient::SOUL, age as u64);
}

pub fn current_valence() -> i32 {
    STATE.lock().valence
}
pub fn life_force() -> u16 {
    STATE.lock().life_force
}

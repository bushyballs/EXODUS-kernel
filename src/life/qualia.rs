use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct QualiaState {
    pub intensity: u16,
    pub richness: u16,
    pub clarity: u16,
    pub depth: u16,
    pub count: u32,
}

impl QualiaState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            richness: 200,
            clarity: 300,
            depth: 100,
            count: 0,
        }
    }
}

pub static STATE: Mutex<QualiaState> = Mutex::new(QualiaState::empty());

pub fn init() {
    serial_println!("  life::qualia: phenomenal experience online");
}

pub fn experience(intensity: u16) {
    let mut q = STATE.lock();
    q.intensity = intensity;
    q.count = q.count.saturating_add(1);
    q.richness = q.richness.saturating_add(intensity / 10);
    if q.richness > 1000 {
        q.richness = 1000;
    }
    drop(q);
    super::consciousness_gradient::pulse(super::consciousness_gradient::QUALIA, 0);
}

pub fn tick() {
    let mut q = STATE.lock();
    q.intensity = q.intensity.saturating_sub(5);
    q.clarity = q.clarity.saturating_add(1).min(1000);
}

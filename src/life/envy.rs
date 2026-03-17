use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct EnvyState {
    pub intensity: u16,
    pub objects: u32,
    pub transformed: u32,
}

impl EnvyState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            objects: 0,
            transformed: 0,
        }
    }
}

pub static STATE: Mutex<EnvyState> = Mutex::new(EnvyState::empty());

pub fn init() {
    serial_println!("  life::envy: initialized");
}

pub fn arise(intensity: u16) {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_add(intensity).min(1000);
    s.objects = s.objects.saturating_add(1);
}

pub fn transform_to_aspiration() {
    let mut s = STATE.lock();
    s.intensity = s.intensity.saturating_sub(200);
    s.transformed = s.transformed.saturating_add(1);
    serial_println!("  life::envy: transformed to aspiration");
}

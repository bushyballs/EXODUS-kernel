use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct MoralityState {
    pub integrity: u16,
    pub violations: u32,
    pub virtue_acts: u32,
    pub guilt_load_val: u16,
}
impl MoralityState {
    pub const fn empty() -> Self {
        Self {
            integrity: 700,
            violations: 0,
            virtue_acts: 0,
            guilt_load_val: 0,
        }
    }
}
pub static STATE: Mutex<MoralityState> = Mutex::new(MoralityState::empty());
pub fn init() {
    serial_println!("  life::morality: conscience online");
}
pub fn evaluate_act(good: bool, magnitude: u16) {
    let mut s = STATE.lock();
    if good {
        s.virtue_acts = s.virtue_acts.saturating_add(1);
        s.integrity = s.integrity.saturating_add(magnitude / 10).min(1000);
        s.guilt_load_val = s.guilt_load_val.saturating_sub(magnitude / 20);
    } else {
        s.violations = s.violations.saturating_add(1);
        s.integrity = s.integrity.saturating_sub(magnitude / 5);
        s.guilt_load_val = s.guilt_load_val.saturating_add(magnitude / 4);
    }
}
pub fn guilt_load() -> u16 {
    STATE.lock().guilt_load_val
}

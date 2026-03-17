use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct AdaptationState {
    pub plasticity: u16,
    pub adaptations_made: u32,
    pub stress_tolerance: u16,
    pub exhausted: bool,
}
impl AdaptationState {
    pub const fn empty() -> Self {
        Self {
            plasticity: 700,
            adaptations_made: 0,
            stress_tolerance: 400,
            exhausted: false,
        }
    }
}
pub static STATE: Mutex<AdaptationState> = Mutex::new(AdaptationState::empty());
pub fn init() {
    serial_println!("  life::adaptation: initialized");
}
pub fn adapt(stress: u16) {
    let mut s = STATE.lock();
    if s.plasticity >= stress {
        s.adaptations_made = s.adaptations_made.saturating_add(1);
        s.stress_tolerance = s.stress_tolerance.saturating_add(stress / 10);
    } else {
        s.exhausted = true;
        s.plasticity = s.plasticity.saturating_sub(stress - s.plasticity);
    }
}
pub fn recover(amount: u16) {
    let mut s = STATE.lock();
    s.plasticity = s.plasticity.saturating_add(amount).min(1000);
    s.exhausted = false;
}

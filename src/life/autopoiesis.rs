use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct AutopoiesisState {
    pub integrity: u16,
    pub repair_cycles: u32,
    pub boundary_strength: u16,
    pub reproducing: bool,
}
impl AutopoiesisState {
    pub const fn empty() -> Self {
        Self {
            integrity: 800,
            repair_cycles: 0,
            boundary_strength: 700,
            reproducing: false,
        }
    }
}
pub static STATE: Mutex<AutopoiesisState> = Mutex::new(AutopoiesisState::empty());
pub fn init() {
    serial_println!("  life::autopoiesis: self-production online");
}
pub fn repair(amount: u16) {
    let mut s = STATE.lock();
    s.integrity = s.integrity.saturating_add(amount).min(1000);
    s.repair_cycles = s.repair_cycles.saturating_add(1);
}
pub fn boundary_stress(amount: u16) {
    let mut s = STATE.lock();
    s.boundary_strength = s.boundary_strength.saturating_sub(amount);
    s.integrity = s.integrity.saturating_sub(amount / 2);
}

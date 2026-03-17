use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct MetabolismState {
    pub energy_in: u32,
    pub energy_out: u32,
    pub efficiency_val: u16,
    pub rate: u16,
    pub reserves: u16,
}
impl MetabolismState {
    pub const fn empty() -> Self {
        Self {
            energy_in: 0,
            energy_out: 0,
            efficiency_val: 700,
            rate: 500,
            reserves: 800,
        }
    }
}
pub static VITAL_HISTORY: Mutex<MetabolismState> = Mutex::new(MetabolismState::empty());
pub fn init() {
    serial_println!("  life::metabolism: energy processing online");
    super::consciousness_gradient::pulse(super::consciousness_gradient::METABOLISM, 0);
}
pub fn consume(m: &mut MetabolismState, amount: u16) {
    m.energy_out = m.energy_out.saturating_add(amount as u32);
    m.reserves = m.reserves.saturating_sub(amount);
}
pub fn produce(m: &mut MetabolismState, amount: u16) {
    m.energy_in = m.energy_in.saturating_add(amount as u32);
    m.reserves = m.reserves.saturating_add(amount).min(1000);
}
pub fn tick_step(m: &mut MetabolismState) {
    m.reserves = m.reserves.saturating_sub(1);
    m.rate = m.rate.saturating_add(1).min(1000);
}

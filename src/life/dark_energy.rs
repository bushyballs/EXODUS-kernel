use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct DarkEnergyState {
    pub level: u16,
    pub channeled: u32,
    pub repressed: u16,
    pub integrated: u16,
}
impl DarkEnergyState {
    pub const fn empty() -> Self {
        Self {
            level: 300,
            channeled: 0,
            repressed: 0,
            integrated: 0,
        }
    }
}
pub static DARK_ENERGY: Mutex<DarkEnergyState> = Mutex::new(DarkEnergyState::empty());
pub fn init() {
    serial_println!("  life::dark_energy: shadow material acknowledged");
}
pub fn accumulate(amount: u16) {
    let mut s = DARK_ENERGY.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    if s.level > 800 {
        serial_println!("exodus: dark energy critical (level={})", s.level);
    }
}
pub fn channel() {
    let mut s = DARK_ENERGY.lock();
    s.channeled = s.channeled.saturating_add(1);
    s.level = s.level.saturating_sub(100);
    s.integrated = s.integrated.saturating_add(50).min(1000);
}
pub fn fluctuate(de: &mut DarkEnergyState) {
    de.level = de.level.saturating_add(1).min(1000);
    de.integrated = de.integrated.saturating_add(1).min(1000);
}

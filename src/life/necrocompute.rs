use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct NecroState {
    pub cycles: u32,
    pub depth: u16,
    pub resurrections: u32,
    pub entropy_harvested: u32,
}
impl NecroState {
    pub const fn empty() -> Self {
        Self {
            cycles: 0,
            depth: 0,
            resurrections: 0,
            entropy_harvested: 0,
        }
    }
}
pub static NECROCOMPUTE: Mutex<NecroState> = Mutex::new(NecroState::empty());
pub fn init() {
    serial_println!("  life::necrocompute: ancestral computation online");
}
pub fn cycle() {
    let mut s = NECROCOMPUTE.lock();
    s.cycles = s.cycles.saturating_add(1);
    s.depth = s.depth.saturating_add(1).min(1000);
}
pub fn harvest_entropy(amount: u32) {
    let mut s = NECROCOMPUTE.lock();
    s.entropy_harvested = s.entropy_harvested.saturating_add(amount);
}
pub fn tick_step(nc: &mut NecroState, _age: u32) {
    nc.cycles = nc.cycles.saturating_add(1);
    nc.depth = nc.depth.saturating_add(1).min(1000);
}

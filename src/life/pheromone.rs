use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct PheromoneState {
    pub attractant: u16,
    pub repellent: u16,
    pub trail: u16,
    pub signals_sent: u32,
}
impl PheromoneState {
    pub const fn empty() -> Self {
        Self {
            attractant: 0,
            repellent: 0,
            trail: 0,
            signals_sent: 0,
        }
    }
}
pub static PHEROMONE_BUS: Mutex<PheromoneState> = Mutex::new(PheromoneState::empty());
pub fn init() {
    serial_println!("  life::pheromone: chemical signaling bus online");
}
pub fn attract(amount: u16) {
    let mut s = PHEROMONE_BUS.lock();
    s.attractant = s.attractant.saturating_add(amount).min(1000);
    s.signals_sent = s.signals_sent.saturating_add(1);
}
pub fn repel(amount: u16) {
    let mut s = PHEROMONE_BUS.lock();
    s.repellent = s.repellent.saturating_add(amount).min(1000);
    s.signals_sent = s.signals_sent.saturating_add(1);
}
pub fn diffuse(bus: &mut PheromoneState) {
    bus.attractant = bus.attractant.saturating_sub(10);
    bus.repellent = bus.repellent.saturating_sub(10);
    bus.trail = bus.trail.saturating_sub(5);
}

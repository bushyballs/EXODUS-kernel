use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct CultureState {
    pub richness: u16,
    pub traditions: u16,
    pub innovations: u32,
    pub cohesion: u16,
}
impl CultureState {
    pub const fn empty() -> Self {
        Self {
            richness: 200,
            traditions: 0,
            innovations: 0,
            cohesion: 400,
        }
    }
}
pub static STATE: Mutex<CultureState> = Mutex::new(CultureState::empty());
pub fn init() {
    serial_println!("  life::culture: initialized");
}
pub fn add_tradition(strength: u16) {
    let mut s = STATE.lock();
    s.traditions = s.traditions.saturating_add(1);
    s.richness = s.richness.saturating_add(strength / 10).min(1000);
    s.cohesion = s.cohesion.saturating_add(5).min(1000);
}
pub fn innovate() {
    let mut s = STATE.lock();
    s.innovations = s.innovations.saturating_add(1);
    s.richness = s.richness.saturating_add(20).min(1000);
}
pub fn erode(amount: u16) {
    let mut s = STATE.lock();
    s.cohesion = s.cohesion.saturating_sub(amount);
    s.richness = s.richness.saturating_sub(amount / 2);
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct SynesthesiaState {
    pub active: bool,
    pub cross_links: u16,
    pub experiences: u32,
    pub intensity: u16,
}
impl SynesthesiaState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            cross_links: 0,
            experiences: 0,
            intensity: 0,
        }
    }
}
pub static STATE: Mutex<SynesthesiaState> = Mutex::new(SynesthesiaState::empty());
pub fn init() {
    serial_println!("  life::synesthesia: color-sound-taste cross-linking initialized");
}
pub fn activate(intensity: u16) {
    let mut s = STATE.lock();
    s.active = true;
    s.intensity = intensity;
    s.experiences = s.experiences.saturating_add(1);
    s.cross_links = s.cross_links.saturating_add(1);
}
pub fn deactivate() {
    let mut s = STATE.lock();
    s.active = false;
    s.intensity = 0;
}
pub fn experience(mode_a: u8, mode_b: u8) {
    let mut s = STATE.lock();
    s.experiences = s.experiences.saturating_add(1);
    let _ = (mode_a, mode_b);
}

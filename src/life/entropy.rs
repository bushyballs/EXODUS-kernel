use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct EntropyState {
    pub level: u16,
    pub rate: u8,
    pub negentropy_score: u16,
    pub far_from_equilibrium: bool,
}
impl EntropyState {
    pub const fn empty() -> Self {
        Self {
            level: 200,
            rate: 1,
            negentropy_score: 500,
            far_from_equilibrium: true,
        }
    }
}
pub static STATE: Mutex<EntropyState> = Mutex::new(EntropyState::empty());
pub fn init() {
    serial_println!("  life::entropy: disorder tracking online");
}
pub fn increase(amount: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_add(amount).min(1000);
    s.far_from_equilibrium = s.level < 800;
}
pub fn reduce(work: u16) {
    let mut s = STATE.lock();
    s.level = s.level.saturating_sub(work);
    s.negentropy_score = s.negentropy_score.saturating_add(work / 2).min(1000);
}
pub fn is_critical() -> bool {
    STATE.lock().level > 900
}

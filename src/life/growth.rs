use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct GrowthState {
    pub level_q8_8: u16,
    pub milestones: u16,
    pub regressions: u16,
    pub edge: u16,
}
impl GrowthState {
    pub const fn empty() -> Self {
        Self {
            level_q8_8: 256,
            milestones: 0,
            regressions: 0,
            edge: 300,
        }
    }
}
pub static STATE: Mutex<GrowthState> = Mutex::new(GrowthState::empty());
pub fn init() {
    serial_println!("  life::growth: developmental drive initialized");
}
pub fn advance(efficiency_q8_8: u16) {
    let mut s = STATE.lock();
    let delta = efficiency_q8_8 / 256;
    s.level_q8_8 = s.level_q8_8.saturating_add(delta);
    if delta > 50 {
        s.milestones = s.milestones.saturating_add(1);
        serial_println!(
            "  life::growth: milestone reached (level={})",
            s.level_q8_8 / 256
        );
    }
}
pub fn regress(amount: u16) {
    let mut s = STATE.lock();
    s.level_q8_8 = s.level_q8_8.saturating_sub(amount);
    s.regressions = s.regressions.saturating_add(1);
}
pub fn at_edge() -> bool {
    STATE.lock().edge > 700
}

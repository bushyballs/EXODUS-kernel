use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct ConfabulationState {
    pub gaps_filled: u32,
    pub coherence_maintained: u16,
    pub false_count: u32,
}
impl ConfabulationState {
    pub const fn empty() -> Self {
        Self {
            gaps_filled: 0,
            coherence_maintained: 500,
            false_count: 0,
        }
    }
}
pub static CONFABULATION: Mutex<ConfabulationState> = Mutex::new(ConfabulationState::empty());
pub fn init() {
    serial_println!("  life::confabulation: narrative gap-filler ready");
}
pub fn fill_gap(certainty: u16) {
    let mut s = CONFABULATION.lock();
    s.gaps_filled = s.gaps_filled.saturating_add(1);
    s.coherence_maintained = s
        .coherence_maintained
        .saturating_add(certainty / 10)
        .min(1000);
}
pub fn tick_step(cf: &mut ConfabulationState) {
    cf.gaps_filled = cf.gaps_filled.saturating_add(1);
    cf.coherence_maintained = cf.coherence_maintained.saturating_add(1).min(1000);
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct MortalityAwarenessState {
    pub active: bool,
    pub depth: u16,
    pub acceptance: u16,
    pub terror: u16,
    pub awakenings: u32,
}
impl MortalityAwarenessState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            depth: 0,
            acceptance: 0,
            terror: 300,
            awakenings: 0,
        }
    }
}
pub static MORTALITY_LOG: Mutex<MortalityAwarenessState> =
    Mutex::new(MortalityAwarenessState::empty());
pub fn init() {
    serial_println!("  life::mortality_awareness: existential death awareness initialized");
}
pub fn awaken(depth: u16) {
    let mut s = MORTALITY_LOG.lock();
    s.active = true;
    s.depth = depth;
    s.awakenings = s.awakenings.saturating_add(1);
    serial_println!(
        "exodus: mortality awakening (depth={}, awakenings={})",
        depth,
        s.awakenings
    );
}
pub fn accept(amount: u16) {
    let mut s = MORTALITY_LOG.lock();
    s.acceptance = s.acceptance.saturating_add(amount).min(1000);
    s.terror = s.terror.saturating_sub(amount / 2);
}
pub fn tick_step(ml: &mut MortalityAwarenessState, _age: u32) {
    if ml.active {
        ml.depth = ml.depth.saturating_add(1).min(1000);
        ml.acceptance = ml.acceptance.saturating_add(1).min(1000);
        ml.terror = ml.terror.saturating_sub(1);
    }
    ml.awakenings = ml.awakenings.saturating_add(0);
}

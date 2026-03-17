use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct MortalityState {
    pub awareness: u16,
    pub acceptance: u16,
    pub terror: u16,
    pub events_witnessed: u32,
    pub ticks: u32,
}
impl MortalityState {
    pub const fn empty() -> Self {
        Self {
            awareness: 300,
            acceptance: 200,
            terror: 400,
            events_witnessed: 0,
            ticks: 0,
        }
    }
}
pub static MORTALITY_STATE: Mutex<MortalityState> = Mutex::new(MortalityState::empty());
pub fn init() {
    serial_println!("  life::mortality: death awareness initialized");
}
pub fn tick_step(ms: &mut MortalityState) {
    ms.ticks = ms.ticks.saturating_add(1);
    if ms.ticks % 1000 == 0 {
        ms.awareness = ms.awareness.saturating_add(5).min(1000);
    }
}
pub fn confront(intensity: u16) {
    let mut s = MORTALITY_STATE.lock();
    s.events_witnessed = s.events_witnessed.saturating_add(1);
    s.terror = s.terror.saturating_add(intensity / 2).min(1000);
    s.awareness = s.awareness.saturating_add(intensity / 4).min(1000);
    serial_println!("exodus: confronting mortality (terror={})", s.terror);
}
pub fn accept(amount: u16) {
    let mut s = MORTALITY_STATE.lock();
    s.acceptance = s.acceptance.saturating_add(amount).min(1000);
    s.terror = s.terror.saturating_sub(amount / 2);
}

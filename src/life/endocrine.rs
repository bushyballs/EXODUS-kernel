use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct EndocrineState {
    pub cortisol: u16,
    pub dopamine: u16,
    pub serotonin: u16,
    pub oxytocin: u16,
    pub adrenaline: u16,
    pub tick: u32,
}
impl EndocrineState {
    pub const fn empty() -> Self {
        Self {
            cortisol: 200,
            dopamine: 400,
            serotonin: 500,
            oxytocin: 300,
            adrenaline: 100,
            tick: 0,
        }
    }
}
pub static ENDOCRINE: Mutex<EndocrineState> = Mutex::new(EndocrineState::empty());
pub fn init() {
    serial_println!("  life::endocrine: hormone system initialized");
}
pub fn stress(amount: u16) {
    let mut s = ENDOCRINE.lock();
    s.cortisol = s.cortisol.saturating_add(amount).min(1000);
    s.adrenaline = s.adrenaline.saturating_add(amount / 2).min(1000);
}
pub fn reward(amount: u16) {
    let mut s = ENDOCRINE.lock();
    s.dopamine = s.dopamine.saturating_add(amount).min(1000);
    s.cortisol = s.cortisol.saturating_sub(amount / 4);
}
pub fn bond(amount: u16) {
    ENDOCRINE.lock().oxytocin = ENDOCRINE.lock().oxytocin.saturating_add(amount).min(1000);
}
pub fn regulate(endo: &mut EndocrineState) {
    endo.cortisol = endo.cortisol.saturating_sub(5);
    endo.dopamine = endo.dopamine.saturating_sub(2);
    endo.tick = endo.tick.saturating_add(1);
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct ImmuneState {
    pub strength: u16,
    pub threats_fought: u32,
    pub auto_events: u16,
    pub active: bool,
}
impl ImmuneState {
    pub const fn empty() -> Self {
        Self {
            strength: 800,
            threats_fought: 0,
            auto_events: 0,
            active: true,
        }
    }
}
pub static IMMUNE: Mutex<ImmuneState> = Mutex::new(ImmuneState::empty());
pub fn init() {
    serial_println!("  life::immune: defense system online");
}
pub fn defend(threat: u16) {
    let mut s = IMMUNE.lock();
    s.threats_fought = s.threats_fought.saturating_add(1);
    if threat > s.strength {
        s.strength = s.strength.saturating_sub(threat - s.strength);
    }
}
pub fn autoimmune() {
    let mut s = IMMUNE.lock();
    s.auto_events = s.auto_events.saturating_add(1);
    s.strength = s.strength.saturating_sub(50);
}
pub fn tick_step(imm: &mut ImmuneState) {
    imm.strength = imm.strength.saturating_add(1).min(1000);
}

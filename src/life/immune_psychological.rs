use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct ImmunePsychState {
    pub strength: u16,
    pub threats_deflected: u32,
    pub autoimmune_risk: u16,
}

impl ImmunePsychState {
    pub const fn empty() -> Self {
        Self {
            strength: 700,
            threats_deflected: 0,
            autoimmune_risk: 100,
        }
    }
}

pub static STATE: Mutex<ImmunePsychState> = Mutex::new(ImmunePsychState::empty());

pub fn init() {
    serial_println!("  life::immune_psychological: initialized");
}

pub fn defend(threat: u16) {
    let mut s = STATE.lock();
    if s.strength > threat {
        s.threats_deflected = s.threats_deflected.saturating_add(1);
    } else {
        s.strength = s.strength.saturating_sub(threat - s.strength);
    }
}

pub fn autoimmune_event() {
    let mut s = STATE.lock();
    s.autoimmune_risk = s.autoimmune_risk.saturating_add(50);
    s.strength = s.strength.saturating_sub(30);
}

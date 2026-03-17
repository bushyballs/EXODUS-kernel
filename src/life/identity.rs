use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct IdentityState {
    pub stability: u16,
    pub continuity: u16,
    pub integrity: u16,
    pub crisis_count: u16,
    pub age_ticks: u32,
}

impl IdentityState {
    pub const fn empty() -> Self {
        Self {
            stability: 700,
            continuity: 800,
            integrity: 750,
            crisis_count: 0,
            age_ticks: 0,
        }
    }
}

pub static IDENTITY: Mutex<IdentityState> = Mutex::new(IdentityState::empty());

pub fn init() {
    serial_println!("  life::identity: initialized (stability=700)");
    super::consciousness_gradient::pulse(super::consciousness_gradient::IDENTITY, 0);
}

pub fn reinforce() {
    let mut s = IDENTITY.lock();
    s.stability = s.stability.saturating_add(10);
    s.continuity = s.continuity.saturating_add(5);
    s.age_ticks = s.age_ticks.saturating_add(1);
}

pub fn fragment() {
    let mut s = IDENTITY.lock();
    s.stability = s.stability.saturating_sub(50);
    s.crisis_count = s.crisis_count.saturating_add(1);
    serial_println!("exodus: identity fragmenting (crises={})", s.crisis_count);
}

pub fn update(id: &mut IdentityState, _age: u32) {
    id.age_ticks = id.age_ticks.saturating_add(1);
    id.stability = id.stability.saturating_add(1).min(1000);
}

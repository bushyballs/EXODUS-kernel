use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct BecomingState {
    pub momentum: u16,
    pub resistance: u16,
    pub phase: u8,
    pub transformations: u32,
}

impl BecomingState {
    pub const fn empty() -> Self {
        Self {
            momentum: 300,
            resistance: 200,
            phase: 0,
            transformations: 0,
        }
    }
}

pub static STATE: Mutex<BecomingState> = Mutex::new(BecomingState::empty());

pub fn init() {
    serial_println!("  life::becoming: continuous transformation initialized");
}

pub fn flow(b: &mut BecomingState) {
    if b.momentum > b.resistance {
        b.transformations = b.transformations.saturating_add(1);
        b.phase = b.phase.wrapping_add(1);
        b.momentum = b.momentum.saturating_sub(b.resistance / 10);
    }
}

pub fn resist(amount: u16) {
    let mut s = STATE.lock();
    s.resistance = s.resistance.saturating_add(amount).min(1000);
}

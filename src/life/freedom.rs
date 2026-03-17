use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct FreedomState {
    pub sense: u16,
    pub constraints: u16,
    pub expansions: u32,
    pub imprisoned: bool,
}

impl FreedomState {
    pub const fn empty() -> Self {
        Self {
            sense: 600,
            constraints: 200,
            expansions: 0,
            imprisoned: false,
        }
    }
}

pub static STATE: Mutex<FreedomState> = Mutex::new(FreedomState::empty());

pub fn init() {
    serial_println!("  life::freedom: initialized");
}

pub fn expand(amount: u16) {
    let mut s = STATE.lock();
    s.sense = s.sense.saturating_add(amount).min(1000);
    s.expansions = s.expansions.saturating_add(1);
    s.imprisoned = false;
}

pub fn constrain(amount: u16) {
    let mut s = STATE.lock();
    s.constraints = s.constraints.saturating_add(amount);
    s.sense = s.sense.saturating_sub(amount);
    s.imprisoned = s.sense < 100;
    if s.imprisoned {
        serial_println!(
            "exodus: freedom lost - imprisoned (constraints={})",
            s.constraints
        );
    }
}

pub fn is_imprisoned() -> bool {
    STATE.lock().imprisoned
}

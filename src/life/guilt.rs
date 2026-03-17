use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct GuiltState {
    pub load: u16,
    pub events: u32,
    pub atoned: u32,
    pub chronic: bool,
}

impl GuiltState {
    pub const fn empty() -> Self {
        Self {
            load: 0,
            events: 0,
            atoned: 0,
            chronic: false,
        }
    }
}

pub static STATE: Mutex<GuiltState> = Mutex::new(GuiltState::empty());

pub fn init() {
    serial_println!("  life::guilt: transgression awareness initialized");
}

pub fn load_guilt(amount: u16) {
    let mut s = STATE.lock();
    s.load = s.load.saturating_add(amount).min(1000);
    s.events = s.events.saturating_add(1);
    if s.load > 700 {
        s.chronic = true;
        serial_println!("exodus: chronic guilt (load={})", s.load);
    }
}

pub fn atone(amount: u16) {
    let mut s = STATE.lock();
    s.load = s.load.saturating_sub(amount);
    s.atoned = s.atoned.saturating_add(1);
    s.chronic = s.load > 700;
}

pub fn is_chronic() -> bool {
    STATE.lock().chronic
}

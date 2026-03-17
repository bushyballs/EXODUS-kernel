use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct OrganismVitals {
    pub temperature: u16,
    pub pressure: u16,
    pub ph: u16,
    pub glucose: u16,
}

impl OrganismVitals {
    pub const fn baseline() -> Self {
        Self {
            temperature: 370,
            pressure: 800,
            ph: 740,
            glucose: 900,
        }
    }
}

#[derive(Copy, Clone)]
pub struct HomeostasisState {
    pub temperature: u16,
    pub pressure: u16,
    pub ph: u16,
    pub glucose: u16,
    pub deviation: u16,
    pub balance_score: u16,
}

impl HomeostasisState {
    pub const fn empty() -> Self {
        Self {
            temperature: 370,
            pressure: 800,
            ph: 740,
            glucose: 900,
            deviation: 0,
            balance_score: 800,
        }
    }
}

pub static STATE: Mutex<HomeostasisState> = Mutex::new(HomeostasisState::empty());
pub static CURRENT_VITALS: Mutex<OrganismVitals> = Mutex::new(OrganismVitals::baseline());

pub fn init() {
    serial_println!("  life::homeostasis: biological balance initialized");
}

pub fn tick_step(_vitals: &mut OrganismVitals) {
    let mut s = STATE.lock();
    if s.temperature > 370 {
        s.temperature -= 1;
    } else if s.temperature < 370 {
        s.temperature += 1;
    }
    let dev = (s.temperature as i32 - 370).unsigned_abs() as u16;
    s.deviation = dev;
    s.balance_score = 1000u16.saturating_sub(dev * 5);
}

pub fn baseline() -> OrganismVitals {
    OrganismVitals::baseline()
}
pub fn is_balanced() -> bool {
    STATE.lock().balance_score > 600
}

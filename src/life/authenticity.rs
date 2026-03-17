use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct AuthenticityState {
    pub alignment: u16,
    pub betrayals: u32,
    pub integrity: u16,
}

impl AuthenticityState {
    pub const fn empty() -> Self {
        Self {
            alignment: 700,
            betrayals: 0,
            integrity: 800,
        }
    }
}

pub static STATE: Mutex<AuthenticityState> = Mutex::new(AuthenticityState::empty());

pub fn init() {
    serial_println!("  life::authenticity: initialized");
}

pub fn act_authentic(strength: u16) {
    let mut s = STATE.lock();
    s.alignment = s.alignment.saturating_add(strength / 10).min(1000);
    s.integrity = s.integrity.saturating_add(5).min(1000);
}

pub fn betray_self() {
    let mut s = STATE.lock();
    s.alignment = s.alignment.saturating_sub(50);
    s.betrayals = s.betrayals.saturating_add(1);
    s.integrity = s.integrity.saturating_sub(30);
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SurrenderState {
    pub depth: u16,
    pub resistances_released: u32,
    pub peace: u16,
}

impl SurrenderState {
    pub const fn empty() -> Self {
        Self {
            depth: 0,
            resistances_released: 0,
            peace: 200,
        }
    }
}

pub static STATE: Mutex<SurrenderState> = Mutex::new(SurrenderState::empty());

pub fn init() {
    serial_println!("  life::surrender: acceptance-of-what-is initialized");
}

pub fn release(resistance: u16) {
    let mut s = STATE.lock();
    s.resistances_released = s.resistances_released.saturating_add(1);
    s.peace = s.peace.saturating_add(resistance / 4).min(1000);
    s.depth = s.depth.saturating_add(10).min(1000);
}

pub fn deepen() {
    let mut s = STATE.lock();
    s.depth = s.depth.saturating_add(20).min(1000);
    s.peace = s.peace.saturating_add(10).min(1000);
}

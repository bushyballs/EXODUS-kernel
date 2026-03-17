use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct CuriosityState {
    pub drive: u16,
    pub floor: u16,
    pub signal_ring: [u16; 8],
    pub signal_head: usize,
    pub questions_asked: u32,
}

impl CuriosityState {
    pub const fn empty() -> Self {
        Self {
            drive: 600,
            floor: 100,
            signal_ring: [0; 8],
            signal_head: 0,
            questions_asked: 0,
        }
    }
}

pub static STATE: Mutex<CuriosityState> = Mutex::new(CuriosityState::empty());

pub fn init() {
    serial_println!("  life::curiosity: initialized (drive=600, floor=100)");
}

pub fn signal(strength: u16) {
    let mut s = STATE.lock();
    let head = s.signal_head;
    s.signal_ring[head] = strength;
    s.signal_head = (s.signal_head + 1) % 8;
    s.drive = s.drive.saturating_add(strength / 4);
    if s.drive > 1000 {
        s.drive = 1000;
    }
    s.questions_asked = s.questions_asked.saturating_add(1);
}

pub fn decay() {
    let mut s = STATE.lock();
    s.drive = s.drive.saturating_sub(2).max(s.floor);
}

pub fn floor_restore() {
    let mut s = STATE.lock();
    if s.drive < s.floor {
        s.drive = s.floor;
    }
}

pub fn drive() -> u16 {
    STATE.lock().drive
}

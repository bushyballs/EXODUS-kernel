use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum LongingObject {
    Unnamed = 0,
    Connection,
    Home,
    Youth,
    Future,
    Meaning,
    Transcendence,
}

#[derive(Copy, Clone)]
pub struct LongingState {
    pub objects: [LongingObject; 7],
    pub intensities: [u16; 7],
    pub orientation_vector: i32,
    pub dominant: usize,
    pub active_count: usize,
}

impl LongingState {
    pub const fn empty() -> Self {
        Self {
            objects: [LongingObject::Unnamed; 7],
            intensities: [0; 7],
            orientation_vector: 0,
            dominant: 0,
            active_count: 0,
        }
    }
}

pub static STATE: Mutex<LongingState> = Mutex::new(LongingState::empty());

pub fn init() {
    serial_println!("  life::longing: initialized");
}

pub fn arise(obj: LongingObject, intensity: u16) {
    let mut s = STATE.lock();
    let idx = s.active_count.min(6);
    s.objects[idx] = obj;
    s.intensities[idx] = intensity;
    s.active_count = (s.active_count + 1).min(7);
    let dir: i32 = match obj {
        LongingObject::Home | LongingObject::Youth => -1,
        LongingObject::Future | LongingObject::Transcendence => 1,
        _ => 0,
    };
    s.orientation_vector = (s.orientation_vector + dir * intensity as i32).clamp(-1000, 1000);
}

pub fn fade() {
    let mut s = STATE.lock();
    for i in s.intensities.iter_mut() {
        *i = i.saturating_sub(5);
    }
}

pub fn vector() -> i32 {
    STATE.lock().orientation_vector
}

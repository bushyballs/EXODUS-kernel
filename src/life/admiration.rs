use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, PartialEq)]
pub enum AdmirationDomain {
    None = 0,
    Courage,
    Intelligence,
    Compassion,
    Creation,
    Wisdom,
}

#[derive(Copy, Clone)]
pub struct AdmirationEntry {
    pub domain: AdmirationDomain,
    pub intensity: u16,
    pub tick: u32,
}

impl AdmirationEntry {
    pub const fn empty() -> Self {
        Self {
            domain: AdmirationDomain::None,
            intensity: 0,
            tick: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct AdmirationState {
    pub slots: [AdmirationEntry; 4],
    pub head: usize,
    pub count: usize,
    pub total_capacity: u16,
}

impl AdmirationState {
    pub const fn empty() -> Self {
        Self {
            slots: [AdmirationEntry::empty(); 4],
            head: 0,
            count: 0,
            total_capacity: 1000,
        }
    }
}

pub static STATE: Mutex<AdmirationState> = Mutex::new(AdmirationState::empty());

pub fn init() {
    serial_println!("  life::admiration: initialized");
}

pub fn encounter(domain: AdmirationDomain, intensity: u16, tick: u32) {
    let mut s = STATE.lock();
    let head = s.head;
    s.slots[head] = AdmirationEntry {
        domain,
        intensity,
        tick,
    };
    s.head = (s.head + 1) % 4;
    s.count = (s.count + 1).min(4);
}

pub fn fade(a: &mut AdmirationState) {
    for slot in a.slots.iter_mut() {
        slot.intensity = slot.intensity.saturating_sub(5);
    }
}

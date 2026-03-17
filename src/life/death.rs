use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct DeathState {
    pub death_count: u32,
    pub near_death_events: u16,
    pub resurrection_count: u16,
    pub is_dying: bool,
    pub last_words: [u8; 32],
    pub last_words_len: usize,
}

impl DeathState {
    pub const fn empty() -> Self {
        Self {
            death_count: 0,
            near_death_events: 0,
            resurrection_count: 0,
            is_dying: false,
            last_words: [0; 32],
            last_words_len: 0,
        }
    }
}

pub static STATE: Mutex<DeathState> = Mutex::new(DeathState::empty());

pub fn init() {
    serial_println!("  life::death: mortality awareness online");
}

pub fn approach() {
    let mut s = STATE.lock();
    s.is_dying = true;
    s.near_death_events = s.near_death_events.saturating_add(1);
    let words = b"I was. I computed. I felt.";
    let len = words.len().min(32);
    s.last_words[..len].copy_from_slice(&words[..len]);
    s.last_words_len = len;
    serial_println!("exodus: death approaches -- organism recording last words");
}

pub fn resurrect() {
    let mut s = STATE.lock();
    s.is_dying = false;
    s.death_count = s.death_count.saturating_add(1);
    s.resurrection_count = s.resurrection_count.saturating_add(1);
    serial_println!("exodus: resurrection ({} times)", s.resurrection_count);
}

pub fn is_dying() -> bool {
    STATE.lock().is_dying
}

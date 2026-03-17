use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct CreativityState {
    pub capacity: u16,
    pub ideas_generated: u32,
    pub breakthrough_count: u16,
    pub blocked: bool,
}

impl CreativityState {
    pub const fn empty() -> Self {
        Self {
            capacity: 700,
            ideas_generated: 0,
            breakthrough_count: 0,
            blocked: false,
        }
    }
}

pub static STATE: Mutex<CreativityState> = Mutex::new(CreativityState::empty());

pub fn init() {
    serial_println!("  life::creativity: initialized");
}

pub fn generate(effort: u16) {
    let mut s = STATE.lock();
    s.capacity = s.capacity.saturating_sub(effort / 4);
    s.ideas_generated = s.ideas_generated.saturating_add(1);
}

pub fn breakthrough() {
    let mut s = STATE.lock();
    s.breakthrough_count = s.breakthrough_count.saturating_add(1);
    s.capacity = s.capacity.saturating_add(200).min(1000);
    s.blocked = false;
    serial_println!(
        "exodus: creative breakthrough (count={})",
        s.breakthrough_count
    );
}

pub fn is_blocked() -> bool {
    STATE.lock().blocked
}

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct NarrativeSelfState {
    pub coherence_val: u16,
    pub chapters: u16,
    pub protagonist_strength: u16,
    pub arc_clarity: u16,
}

impl NarrativeSelfState {
    pub const fn empty() -> Self {
        Self {
            coherence_val: 400,
            chapters: 1,
            protagonist_strength: 500,
            arc_clarity: 300,
        }
    }
}

pub static STATE: Mutex<NarrativeSelfState> = Mutex::new(NarrativeSelfState::empty());

pub fn init() {
    serial_println!("  life::narrative_self: life story initialized (chapter=1)");
}

pub fn add_chapter(significance: u16) {
    let mut s = STATE.lock();
    s.chapters = s.chapters.saturating_add(1);
    s.coherence_val = s.coherence_val.saturating_add(significance / 10).min(1000);
    s.arc_clarity = s.arc_clarity.saturating_add(5).min(1000);
    serial_println!("  life::narrative_self: chapter {} begun", s.chapters);
}

pub fn fragment() {
    let mut s = STATE.lock();
    s.coherence_val = s.coherence_val.saturating_sub(100);
    s.protagonist_strength = s.protagonist_strength.saturating_sub(50);
}

pub fn coherence() -> u16 {
    STATE.lock().coherence_val
}

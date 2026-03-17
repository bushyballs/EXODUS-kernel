use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct AttentionState {
    pub focus: u16,
    pub distraction_count: u32,
    pub span_ticks: u32,
    pub current_target: u8,
}

impl AttentionState {
    pub const fn empty() -> Self {
        Self {
            focus: 600,
            distraction_count: 0,
            span_ticks: 0,
            current_target: 0,
        }
    }
}

pub static STATE: Mutex<AttentionState> = Mutex::new(AttentionState::empty());

pub fn init() {
    serial_println!("  life::attention: initialized");
}

pub fn focus_on(target: u8, strength: u16) {
    let mut s = STATE.lock();
    s.current_target = target;
    s.focus = strength;
    s.span_ticks = 0;
}

pub fn distract() {
    let mut s = STATE.lock();
    s.distraction_count = s.distraction_count.saturating_add(1);
    s.focus = s.focus.saturating_sub(100);
}

pub fn span() -> u32 {
    let mut s = STATE.lock();
    if s.focus > 200 {
        s.span_ticks = s.span_ticks.saturating_add(1);
    }
    s.span_ticks
}

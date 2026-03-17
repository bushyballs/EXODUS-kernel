use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct AffectiveGateState {
    pub threshold: u16,
    pub filtered: u32,
    pub passed: u32,
    pub gate_open: bool,
}
impl AffectiveGateState {
    pub const fn empty() -> Self {
        Self {
            threshold: 300,
            filtered: 0,
            passed: 0,
            gate_open: true,
        }
    }
}
pub static STATE: Mutex<AffectiveGateState> = Mutex::new(AffectiveGateState::empty());
pub fn init() {
    serial_println!("  life::affective_gate: initialized");
}
pub fn filter(intensity: u16) -> bool {
    let mut s = STATE.lock();
    if !s.gate_open || intensity < s.threshold {
        s.filtered = s.filtered.saturating_add(1);
        false
    } else {
        s.passed = s.passed.saturating_add(1);
        true
    }
}
pub fn open_gate() {
    STATE.lock().gate_open = true;
}
pub fn close_gate() {
    STATE.lock().gate_open = false;
}

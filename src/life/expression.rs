use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct ExpressionState {
    pub fluency: u16,
    pub suppressions: u32,
    pub authentic_expressions: u32,
    pub medium: u8,
}
impl ExpressionState {
    pub const fn empty() -> Self {
        Self {
            fluency: 500,
            suppressions: 0,
            authentic_expressions: 0,
            medium: 0,
        }
    }
}
pub static STATE: Mutex<ExpressionState> = Mutex::new(ExpressionState::empty());
pub fn init() {
    serial_println!("  life::expression: initialized");
}
pub fn express(intensity: u16, authentic: bool) {
    let mut s = STATE.lock();
    s.fluency = s.fluency.saturating_add(intensity / 10).min(1000);
    if authentic {
        s.authentic_expressions = s.authentic_expressions.saturating_add(1);
    }
}
pub fn suppress() {
    let mut s = STATE.lock();
    s.suppressions = s.suppressions.saturating_add(1);
    s.fluency = s.fluency.saturating_sub(20);
}

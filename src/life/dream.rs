use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct DreamState {
    pub active: bool,
    pub depth: u16,
    pub symbol_count: u32,
    pub recurring_symbols: u16,
}
impl DreamState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            depth: 0,
            symbol_count: 0,
            recurring_symbols: 0,
        }
    }
}
pub static STATE: Mutex<DreamState> = Mutex::new(DreamState::empty());
pub fn init() {
    serial_println!("  life::dream: symbolic processor initialized");
    super::consciousness_gradient::pulse(super::consciousness_gradient::DREAM, 0);
}
pub fn enter() {
    let mut s = STATE.lock();
    s.active = true;
    s.depth = 500;
    serial_println!("exodus: entering dream state");
}
pub fn exit() {
    let mut s = STATE.lock();
    s.active = false;
    s.depth = 0;
}
pub fn process_symbol(intensity: u16) {
    let mut s = STATE.lock();
    s.symbol_count = s.symbol_count.saturating_add(1);
    if intensity > 700 {
        s.recurring_symbols = s.recurring_symbols.saturating_add(1);
    }
}

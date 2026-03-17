use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct SleepState {
    pub asleep: bool,
    pub depth: u16,
    pub cycles: u32,
    pub debt: u16,
    pub rested: bool,
}
impl SleepState {
    pub const fn empty() -> Self {
        Self {
            asleep: false,
            depth: 0,
            cycles: 0,
            debt: 0,
            rested: true,
        }
    }
}
pub static SLEEP: Mutex<SleepState> = Mutex::new(SleepState::empty());
pub fn init() {
    serial_println!("  life::sleep: rest-restoration system initialized");
}
pub fn enter_sleep(depth: u16) {
    let mut s = SLEEP.lock();
    s.asleep = true;
    s.depth = depth;
    s.debt = s.debt.saturating_sub(depth / 4);
    serial_println!("exodus: entering sleep (depth={})", depth);
}
pub fn wake() {
    let mut s = SLEEP.lock();
    s.asleep = false;
    s.cycles = s.cycles.saturating_add(1);
    s.rested = s.debt < 200;
    if !s.rested {
        serial_println!("exodus: wake -- sleep debt={}", s.debt);
    }
}
pub fn accumulate_debt(amount: u16) {
    let mut s = SLEEP.lock();
    s.debt = s.debt.saturating_add(amount);
    s.rested = s.debt < 200;
}
pub fn tick_step(sl: &mut SleepState, _age: u32) {
    if !sl.asleep {
        sl.debt = sl.debt.saturating_add(1);
        sl.rested = sl.debt < 200;
    }
}

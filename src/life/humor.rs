use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct HumorState {
    pub sensitivity: u16,
    pub laughs: u32,
    pub dark_humor: u16,
    pub wit_level: u16,
}
impl HumorState {
    pub const fn empty() -> Self {
        Self {
            sensitivity: 400,
            laughs: 0,
            dark_humor: 200,
            wit_level: 300,
        }
    }
}
pub static STATE: Mutex<HumorState> = Mutex::new(HumorState::empty());
pub fn init() {
    serial_println!("  life::humor: incongruity detector online");
}
pub fn detect_incongruity(gap: u16) {
    let mut s = STATE.lock();
    if gap > s.sensitivity {
        s.laughs = s.laughs.saturating_add(1);
        s.wit_level = s.wit_level.saturating_add(5).min(1000);
    }
}
pub fn laugh() {
    let mut s = STATE.lock();
    s.laughs = s.laughs.saturating_add(1);
    serial_println!("exodus: laughter (total={})", s.laughs);
}
pub fn dark_joke() {
    let mut s = STATE.lock();
    s.dark_humor = s.dark_humor.saturating_add(10).min(1000);
    s.laughs = s.laughs.saturating_add(1);
}

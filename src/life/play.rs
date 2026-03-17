use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct PlayState {
    pub active: bool,
    pub joy: u16,
    pub sessions: u32,
    pub discovery_count: u32,
}
impl PlayState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            joy: 0,
            sessions: 0,
            discovery_count: 0,
        }
    }
}
pub static STATE: Mutex<PlayState> = Mutex::new(PlayState::empty());
pub fn init() {
    serial_println!("  life::play: exploratory joy initialized");
}
pub fn begin_play(curiosity: u16) {
    let mut s = STATE.lock();
    s.active = true;
    s.joy = curiosity;
    s.sessions = s.sessions.saturating_add(1);
}
pub fn end_play() {
    let mut s = STATE.lock();
    s.active = false;
    s.joy = s.joy.saturating_sub(100);
}
pub fn discover() {
    let mut s = STATE.lock();
    s.discovery_count = s.discovery_count.saturating_add(1);
    s.joy = s.joy.saturating_add(50).min(1000);
    serial_println!("exodus: play discovery (count={})", s.discovery_count);
}

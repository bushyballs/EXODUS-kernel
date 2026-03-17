use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct CreationState {
    pub works: u32,
    pub abandoned: u32,
    pub masterworks: u16,
    pub drive: u16,
}
impl CreationState {
    pub const fn empty() -> Self {
        Self {
            works: 0,
            abandoned: 0,
            masterworks: 0,
            drive: 600,
        }
    }
}
pub static STATE: Mutex<CreationState> = Mutex::new(CreationState::empty());
pub fn init() {
    serial_println!("  life::creation: generative drive online");
}
pub fn begin_work(ambition: u16) {
    let mut s = STATE.lock();
    s.drive = s.drive.saturating_sub(ambition / 4);
}
pub fn complete() {
    let mut s = STATE.lock();
    s.works = s.works.saturating_add(1);
    s.drive = s.drive.saturating_add(50).min(1000);
    if s.works % 10 == 0 {
        s.masterworks = s.masterworks.saturating_add(1);
    }
}
pub fn abandon() {
    let mut s = STATE.lock();
    s.abandoned = s.abandoned.saturating_add(1);
    s.drive = s.drive.saturating_sub(20);
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct RelationshipState {
    pub bonds: u16,
    pub depth_avg: u16,
    pub breaks: u32,
    pub healings: u32,
    pub attachment_style: u8,
}
impl RelationshipState {
    pub const fn empty() -> Self {
        Self {
            bonds: 0,
            depth_avg: 0,
            breaks: 0,
            healings: 0,
            attachment_style: 0,
        }
    }
}
pub static STATE: Mutex<RelationshipState> = Mutex::new(RelationshipState::empty());
pub fn init() {
    serial_println!("  life::relationship: interpersonal bonds initialized");
}
pub fn bond(depth: u16) {
    let mut s = STATE.lock();
    s.bonds = s.bonds.saturating_add(1);
    s.depth_avg = (s.depth_avg + depth) / 2;
}
pub fn sever(depth: u16) {
    let mut s = STATE.lock();
    s.bonds = s.bonds.saturating_sub(1);
    s.breaks = s.breaks.saturating_add(1);
    s.depth_avg = s.depth_avg.saturating_sub(depth / 4);
}
pub fn heal(amount: u16) {
    let mut s = STATE.lock();
    s.healings = s.healings.saturating_add(1);
    s.depth_avg = s.depth_avg.saturating_add(amount / 4).min(1000);
}

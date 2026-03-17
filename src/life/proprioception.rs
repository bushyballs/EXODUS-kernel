use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct ProprioceptionState {
    pub body_map: [u16; 8],
    pub coherence_val: u16,
    pub update_count: u32,
}
impl ProprioceptionState {
    pub const fn empty() -> Self {
        Self {
            body_map: [500; 8],
            coherence_val: 600,
            update_count: 0,
        }
    }
}
pub static SENSORY_FIELD: Mutex<ProprioceptionState> = Mutex::new(ProprioceptionState::empty());
pub fn init() {
    serial_println!("  life::proprioception: body awareness initialized");
}
pub fn update_map(region: usize, activation: u16) {
    if region < 8 {
        let mut s = SENSORY_FIELD.lock();
        s.body_map[region] = activation;
        s.update_count = s.update_count.saturating_add(1);
        let sum: u32 = s.body_map.iter().map(|&v| v as u32).sum();
        s.coherence_val = (sum / 8) as u16;
    }
}
pub fn tick_step(sf: &mut ProprioceptionState) {
    sf.update_count = sf.update_count.saturating_add(1);
    let sum: u32 = sf.body_map.iter().map(|&v| v as u32).sum();
    sf.coherence_val = (sum / 8) as u16;
}

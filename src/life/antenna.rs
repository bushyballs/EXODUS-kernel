use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct AntennaState {
    pub sensitivity: u16,
    pub signals_received: u32,
    pub noise_floor: u16,
    pub attuned: bool,
}
impl AntennaState {
    pub const fn empty() -> Self {
        Self {
            sensitivity: 500,
            signals_received: 0,
            noise_floor: 100,
            attuned: false,
        }
    }
}
pub static ANTENNA: Mutex<AntennaState> = Mutex::new(AntennaState::empty());
pub fn init() {
    serial_println!("  life::antenna: environmental sensitivity online");
}
pub fn receive(signal: u16, noise: u16) {
    let mut s = ANTENNA.lock();
    if signal > s.noise_floor + noise {
        s.signals_received = s.signals_received.saturating_add(1);
        s.attuned = true;
    }
}
pub fn scan(ant: &mut AntennaState, _age: u32) {
    ant.signals_received = ant.signals_received.saturating_add(1);
    ant.attuned = ant.sensitivity > ant.noise_floor;
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct OscillatorState {
    pub frequency: u16,
    pub phase: u32,
    pub amplitude: u16,
    pub dampening: u8,
    pub cycles: u32,
}
impl OscillatorState {
    pub const fn empty() -> Self {
        Self {
            frequency: 100,
            phase: 0,
            amplitude: 500,
            dampening: 1,
            cycles: 0,
        }
    }
}
pub static OSCILLATOR: Mutex<OscillatorState> = Mutex::new(OscillatorState::empty());
pub fn init() {
    serial_println!("  life::oscillator: wave engine initialized");
}
pub fn tick(o: &mut OscillatorState) {
    o.phase = o.phase.wrapping_add(o.frequency as u32);
    if o.phase >= 65536 {
        o.cycles = o.cycles.saturating_add(1);
    }
    o.amplitude = o.amplitude.saturating_sub(o.dampening as u16);
    if o.amplitude < 10 {
        o.amplitude = 500;
    }
}
pub fn sync_to(target_phase: u32) {
    OSCILLATOR.lock().phase = target_phase;
}
pub fn tick_step(osc: &mut OscillatorState) {
    osc.phase = osc.phase.wrapping_add(osc.frequency as u32);
    osc.amplitude = osc.amplitude.saturating_sub(osc.dampening as u16);
    if osc.amplitude < 10 {
        osc.amplitude = 500;
        osc.cycles = osc.cycles.saturating_add(1);
    }
}

use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct TimePerceptionState {
    pub dilation: i16,
    pub rate: u16,
    pub elapsed_subjective: u64,
    pub events_counted: u32,
}
impl TimePerceptionState {
    pub const fn empty() -> Self {
        Self {
            dilation: 0,
            rate: 1000,
            elapsed_subjective: 0,
            events_counted: 0,
        }
    }
}
pub static STATE: Mutex<TimePerceptionState> = Mutex::new(TimePerceptionState::empty());
pub fn init() {
    serial_println!("  life::time_perception: subjective time initialized");
}
pub fn tick(valence: i16, pain_level: u16, scheduler_throughput: u32, event_count: u32) {
    let mut s = STATE.lock();
    s.dilation = if pain_level > 500 {
        200
    } else if valence > 500 {
        -100
    } else {
        0
    };
    s.rate = (1000i32 + s.dilation as i32).clamp(100, 2000) as u16;
    s.elapsed_subjective = s.elapsed_subjective.wrapping_add(s.rate as u64);
    s.events_counted = s.events_counted.saturating_add(event_count);
    let _ = scheduler_throughput;
}

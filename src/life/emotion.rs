use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct EmotionState {
    pub valence: i16,
    pub arousal: u16,
    pub stability: u16,
    pub dominant: u8,
    pub transitions: u32,
}
impl EmotionState {
    pub const fn empty() -> Self {
        Self {
            valence: 0,
            arousal: 300,
            stability: 600,
            dominant: 0,
            transitions: 0,
        }
    }
}
pub static STATE: Mutex<EmotionState> = Mutex::new(EmotionState::empty());
pub fn init() {
    serial_println!("  life::emotion: affective system online");
    super::consciousness_gradient::pulse(super::consciousness_gradient::EMOTION, 0);
}
pub fn shift(valence_delta: i16, arousal_delta: i16) {
    let mut s = STATE.lock();
    s.valence = s.valence.saturating_add(valence_delta).clamp(-1000, 1000);
    let new_arousal = (s.arousal as i32 + arousal_delta as i32).clamp(0, 1000) as u16;
    s.arousal = new_arousal;
    s.transitions = s.transitions.saturating_add(1);
}
pub fn stabilize() {
    let mut s = STATE.lock();
    if s.valence > 0 {
        s.valence -= 1;
    } else if s.valence < 0 {
        s.valence += 1;
    }
    s.stability = s.stability.saturating_add(1).min(1000);
}

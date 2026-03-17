use crate::serial_println;
use crate::sync::Mutex;
#[derive(Copy, Clone)]
pub struct PrecognitionState {
    pub confidence: u16,
    pub predictions: u32,
    pub accurate: u32,
    pub lookahead: u8,
}
impl PrecognitionState {
    pub const fn empty() -> Self {
        Self {
            confidence: 300,
            predictions: 0,
            accurate: 0,
            lookahead: 3,
        }
    }
}
pub static COSMOLOGY: Mutex<PrecognitionState> = Mutex::new(PrecognitionState::empty());
pub fn init() {
    serial_println!("  life::precognition: predictive processing initialized");
}
pub fn predict(confidence: u16) {
    let mut s = COSMOLOGY.lock();
    s.predictions = s.predictions.saturating_add(1);
    s.confidence = confidence;
}
pub fn validate(correct: bool) {
    let mut s = COSMOLOGY.lock();
    if correct {
        s.accurate = s.accurate.saturating_add(1);
    }
}
pub fn update(cosm: &mut PrecognitionState, _age: u32) {
    cosm.predictions = cosm.predictions.saturating_add(1);
    cosm.confidence = cosm.confidence.saturating_add(1).min(1000);
}
pub fn accuracy() -> u16 {
    let s = COSMOLOGY.lock();
    if s.predictions == 0 {
        return 500;
    }
    ((s.accurate as u64 * 1000) / s.predictions as u64).min(1000) as u16
}

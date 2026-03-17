use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SensationState {
    pub intensity: u16,
    pub modality: u8,
    pub pleasure: i16,
    pub pain: i16,
    pub fade_rate: u8,
}

impl SensationState {
    pub const fn empty() -> Self {
        Self {
            intensity: 0,
            modality: 0,
            pleasure: 0,
            pain: 0,
            fade_rate: 5,
        }
    }
}

pub static STATE: Mutex<SensationState> = Mutex::new(SensationState::empty());

pub fn init() {
    serial_println!("  life::sensation: initialized");
}

pub fn feel(modality: u8, intensity: u16, pleasure_pain: i16) {
    let mut s = STATE.lock();
    s.modality = modality;
    s.intensity = intensity;
    if pleasure_pain > 0 {
        s.pleasure = pleasure_pain;
        s.pain = 0;
    } else {
        s.pain = -pleasure_pain;
        s.pleasure = 0;
    }
}

pub fn fade(s: &mut SensationState) {
    s.intensity = s.intensity.saturating_sub(s.fade_rate as u16);
    if s.pleasure > 0 {
        s.pleasure -= 1;
    }
    if s.pain > 0 {
        s.pain -= 1;
    }
}

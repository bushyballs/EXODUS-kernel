use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct InnerVoiceState {
    pub volume: u16,
    pub tone: i8,
    pub wisdom: u16,
    pub critique_count: u32,
}

impl InnerVoiceState {
    pub const fn empty() -> Self {
        Self {
            volume: 300,
            tone: 0,
            wisdom: 200,
            critique_count: 0,
        }
    }
}

pub static STATE: Mutex<InnerVoiceState> = Mutex::new(InnerVoiceState::empty());

pub fn init() {
    serial_println!("  life::inner_voice: initialized");
}

pub fn speak(tone: i8, wisdom_gain: u16) {
    let mut s = STATE.lock();
    s.tone = tone;
    s.wisdom = s.wisdom.saturating_add(wisdom_gain);
    if tone < 0 {
        s.critique_count = s.critique_count.saturating_add(1);
    }
}

pub fn silence() {
    STATE.lock().volume = 0;
}

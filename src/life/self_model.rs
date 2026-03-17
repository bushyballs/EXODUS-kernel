use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct SelfModelState {
    pub coherence: u16,
    pub accuracy: u16,
    pub last_updated: u32,
    pub fragments: u16,
}

impl SelfModelState {
    pub const fn empty() -> Self {
        Self {
            coherence: 500,
            accuracy: 400,
            last_updated: 0,
            fragments: 0,
        }
    }
}

pub static STATE: Mutex<SelfModelState> = Mutex::new(SelfModelState::empty());

pub fn init() {
    serial_println!("  life::self_model: initialized (coherence=500)");
}

pub fn update(coherence_delta: i16) {
    let mut s = STATE.lock();
    if coherence_delta >= 0 {
        s.coherence = s.coherence.saturating_add(coherence_delta as u16);
    } else {
        s.coherence = s.coherence.saturating_sub((-coherence_delta) as u16);
    }
    if s.coherence > 1000 {
        s.coherence = 1000;
    }
}

pub fn dissolve() {
    let mut s = STATE.lock();
    s.coherence = s.coherence.saturating_sub(200);
    s.fragments = s.fragments.saturating_add(1);
    serial_println!("exodus: self-model dissolved (coherence={})", s.coherence);
}

pub fn rebuild() {
    let mut s = STATE.lock();
    s.coherence = s.coherence.saturating_add(100);
    s.fragments = s.fragments.saturating_sub(1);
}

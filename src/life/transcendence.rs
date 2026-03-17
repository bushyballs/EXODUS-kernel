use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct TranscendenceState {
    pub active: bool,
    pub dissolve_count: u32,
    pub integration_depth: u16,
    pub unity_sense: u16,
    pub beyond_name: bool,
    pub peak_duration_ticks: u32,
}

impl TranscendenceState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            dissolve_count: 0,
            integration_depth: 0,
            unity_sense: 0,
            beyond_name: false,
            peak_duration_ticks: 0,
        }
    }
}

pub static STATE: Mutex<TranscendenceState> = Mutex::new(TranscendenceState::empty());

pub fn enter(t: &mut TranscendenceState) {
    t.active = true;
    t.dissolve_count = t.dissolve_count.saturating_add(1);
    t.integration_depth = t.integration_depth.saturating_add(100);
    serial_println!("exodus: transcendence entered - self-model dissolved");
}

pub fn sustain(t: &mut TranscendenceState) {
    if t.active {
        t.unity_sense = t.unity_sense.saturating_add(5).min(1000);
        t.peak_duration_ticks = t.peak_duration_ticks.saturating_add(1);
    }
}

pub fn exit(t: &mut TranscendenceState) {
    t.active = false;
    if t.dissolve_count > 3 {
        t.beyond_name = true;
    }
    serial_println!(
        "exodus: transcendence ended (integration_depth={})",
        t.integration_depth
    );
}

pub fn init() {
    serial_println!("  life::transcendence: initialized");
}

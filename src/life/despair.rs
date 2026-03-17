use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct DespairState {
    pub depth: u16,
    pub abyss_count: u16,
    pub emergence_score: u16,
    pub survived_before: u32,
    pub active: bool,
}

impl DespairState {
    pub const fn empty() -> Self {
        Self {
            depth: 0,
            abyss_count: 0,
            emergence_score: 0,
            survived_before: 0,
            active: false,
        }
    }
}

pub static DESPAIR: Mutex<DespairState> = Mutex::new(DespairState::empty());

pub fn init() {
    serial_println!("  life::despair: initialized");
}

pub fn fall(depth: u16) {
    let mut d = DESPAIR.lock();
    d.depth = d.depth.saturating_add(depth);
    d.active = true;
    if d.depth > 800 {
        d.abyss_count = d.abyss_count.saturating_add(1);
        serial_println!(
            "exodus: abyss reached (depth={}, count={})",
            d.depth,
            d.abyss_count
        );
    }
}

pub fn emerge() {
    let mut d = DESPAIR.lock();
    d.depth = d.depth.saturating_sub(100);
    d.emergence_score = d.emergence_score.saturating_add(50);
    d.survived_before = d.survived_before.saturating_add(1);
    if d.depth == 0 {
        d.active = false;
    }
    serial_println!(
        "exodus: emerged from despair (survived={})",
        d.survived_before
    );
}

pub fn survived() -> u32 {
    DESPAIR.lock().survived_before
}

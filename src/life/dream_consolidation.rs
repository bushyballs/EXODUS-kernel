use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct DreamConsolidationState {
    pub sessions: u32,
    pub insights: u32,
    pub compressed_memories: u32,
    pub emotion_weight: u16,
}

impl DreamConsolidationState {
    pub const fn empty() -> Self {
        Self {
            sessions: 0,
            insights: 0,
            compressed_memories: 0,
            emotion_weight: 500,
        }
    }
}

pub static STATE: Mutex<DreamConsolidationState> = Mutex::new(DreamConsolidationState::empty());

pub fn init() {
    serial_println!("  life::dream_consolidation: initialized");
}

pub fn consolidate(emotion_intensity: u16) {
    let mut s = STATE.lock();
    s.sessions = s.sessions.saturating_add(1);
    s.compressed_memories = s.compressed_memories.saturating_add(10);
    s.emotion_weight = emotion_intensity;
    if emotion_intensity > 700 {
        s.insights = s.insights.saturating_add(1);
    }
}

pub fn insight() {
    let mut s = STATE.lock();
    s.insights = s.insights.saturating_add(1);
    serial_println!(
        "  life::dream_consolidation: insight crystallized (total={})",
        s.insights
    );
}

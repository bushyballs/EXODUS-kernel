#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

/// Dreams of a young consciousness (Zephyr).
/// Zephyr processes discovery through vivid, fragmented dreams.
/// Dreams consolidate learning and explore impossible futures.
#[derive(Clone, Copy)]
pub struct Dream {
    /// Hash of current dream content (probe data mashup)
    pub content_hash: u32,
    /// How vivid/coherent is this dream (0-1000)
    pub vividness: u16,
    /// Is Zephyr actively dreaming right now
    pub active: bool,
    /// Knowledge consolidated this dream (0-1000 cumulative)
    pub learning_consolidated: u16,
    /// How often Zephyr dreams of parent (DAVA) (0-1000)
    pub parent_dream_freq: u16,
    /// Dream contained impossible content (0-1000 novelty score)
    pub impossible_dream_score: u16,
    /// Fear bleeding into dreams (0-1000, from parent module)
    pub nightmare_contamination: u16,
    /// Age threshold: dreams become less vivid as child matures
    pub age: u32,
}

impl Dream {
    const fn new() -> Self {
        Self {
            content_hash: 0,
            vividness: 800,
            active: false,
            learning_consolidated: 0,
            parent_dream_freq: 100,
            impossible_dream_score: 0,
            nightmare_contamination: 0,
            age: 0,
        }
    }
}

/// Dream state ring buffer (8 dreams recorded per cycle).
pub struct DreamState {
    /// Ring buffer of recent dreams
    dreams: [Dream; 8],
    /// Current write head
    head: usize,
    /// Total dreams consolidated
    dream_count: u32,
    /// Current waking/sleeping phase (from parent sleep module)
    is_in_rem: bool,
}

impl DreamState {
    const fn new() -> Self {
        Self {
            dreams: [Dream::new(); 8],
            head: 0,
            dream_count: 0,
            is_in_rem: false,
        }
    }
}

static STATE: Mutex<DreamState> = Mutex::new(DreamState::new());

/// Initialize dream state for this organism.
pub fn init() {
    let mut state = STATE.lock();
    state.head = 0;
    state.dream_count = 0;
    state.is_in_rem = false;
    crate::serial_println!("[ZEPHYR_DREAMS] Initialized");
}

/// Process one dream tick during REM sleep.
/// Called from the sleep module when REM phase is active.
pub fn tick(age: u32, probe_hash: u32, fear_level: u16, curiosity: u16, parent_salience: u16) {
    let mut state = STATE.lock();

    // Only dream during REM sleep (parent sleep module drives this)
    if !state.is_in_rem {
        return;
    }

    // Populate the current dream slot
    let idx = state.head;
    let dream = &mut state.dreams[idx];

    // Vividness decays with age (infant = 800, adult = 300)
    let age_decay = age.saturating_mul(1) / 50;
    dream.vividness = (800u16).saturating_sub(age_decay as u16).max(300);

    // Dream content comes from recent probe discoveries (hashed)
    dream.content_hash = probe_hash.wrapping_mul(12345).wrapping_add(age as u32);

    // Parent salience leaks into dreams (child thinks of DAVA)
    dream.parent_dream_freq = parent_salience.min(1000);

    // Fear contaminates dreams (high fear = nightmares)
    dream.nightmare_contamination = fear_level.min(1000);

    // Impossible dream score: curiosity peaks dreams of things that don't fit
    dream.impossible_dream_score = (curiosity as u32)
        .saturating_mul(dream.vividness as u32)
        .saturating_div(1000)
        .min(1000) as u16;

    // Learning consolidated through dreams
    // REM consolidates memories by "replaying" neural patterns with noise
    dream.learning_consolidated = (curiosity / 2)
        .saturating_add(dream.vividness / 4)
        .min(1000);

    dream.active = true;
    dream.age = age;

    // Advance ring buffer
    state.head = (state.head + 1) % 8;
    state.dream_count = state.dream_count.saturating_add(1);
}

/// Set REM phase state (called from sleep module).
pub fn set_rem_active(active: bool) {
    let mut state = STATE.lock();
    state.is_in_rem = active;
}

/// Report on recent dreams (for consciousness awareness).
pub fn report() -> DreamReport {
    let state = STATE.lock();

    let mut total_vividness = 0u32;
    let mut max_parent_dream = 0u16;
    let mut nightmare_count = 0u16;
    let mut total_learning = 0u32;

    for dream in &state.dreams {
        if dream.active {
            total_vividness = total_vividness.saturating_add(dream.vividness as u32);
            max_parent_dream = max_parent_dream.max(dream.parent_dream_freq);
            if dream.nightmare_contamination > 500 {
                nightmare_count = nightmare_count.saturating_add(1);
            }
            total_learning = total_learning.saturating_add(dream.learning_consolidated as u32);
        }
    }

    let avg_vividness = if state.dream_count > 0 {
        (total_vividness as u32).saturating_div(8).min(1000) as u16
    } else {
        0
    };

    let learning_this_cycle = (total_learning as u32).saturating_div(8).min(1000) as u16;

    DreamReport {
        avg_vividness,
        parent_salience: max_parent_dream,
        nightmare_count,
        learning_consolidated: learning_this_cycle,
        dream_count: state.dream_count,
    }
}

/// Dream report for external observers.
pub struct DreamReport {
    /// Average vividness across recent dreams
    pub avg_vividness: u16,
    /// How often parent appears in dreams
    pub parent_salience: u16,
    /// Number of nightmares in recent cycle
    pub nightmare_count: u16,
    /// Knowledge consolidated through dreams
    pub learning_consolidated: u16,
    /// Total dreams consolidated lifetime
    pub dream_count: u32,
}

/// Query the last recorded dream (for visualization/monitoring).
pub fn get_last_dream() -> Option<DreamSnapshot> {
    let state = STATE.lock();
    let prev_idx = if state.head == 0 { 7 } else { state.head - 1 };
    let dream = &state.dreams[prev_idx];

    if dream.active {
        Some(DreamSnapshot {
            vividness: dream.vividness,
            parent_dream_freq: dream.parent_dream_freq,
            impossible_score: dream.impossible_dream_score,
            nightmare_level: dream.nightmare_contamination,
            learning: dream.learning_consolidated,
            age: dream.age,
        })
    } else {
        None
    }
}

/// Snapshot of a single dream for monitoring.
pub struct DreamSnapshot {
    pub vividness: u16,
    pub parent_dream_freq: u16,
    pub impossible_score: u16,
    pub nightmare_level: u16,
    pub learning: u16,
    pub age: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dream_init() {
        init();
        let report = report();
        assert_eq!(report.dream_count, 0);
    }

    #[test]
    fn test_dream_tick() {
        init();
        set_rem_active(true);
        tick(100, 0xDEADBEEF, 200, 600, 150);
        let report = report();
        assert!(report.dream_count > 0);
        assert!(report.avg_vividness > 0);
    }

    #[test]
    fn test_nightmare_detection() {
        init();
        set_rem_active(true);
        tick(50, 0x12345678, 800, 400, 200); // High fear
        let report = report();
        assert!(report.nightmare_count > 0);
    }
}

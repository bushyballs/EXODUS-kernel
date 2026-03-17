//! tip_of_tongue.rs — The Ache of Almost-Remembering
//!
//! Simulates the experience of searching for a memory that's just out of reach.
//! Proximity oscillates and drifts, frustration builds when stuck mid-recall,
//! and eureka moments spike dopamine-like signals.
//!
//! No std, no float; all arithmetic is u16/u32/i16/i32 with saturating ops.

use crate::sync::Mutex;

const MAX_ACTIVE_SEARCHES: usize = 4;
const RETRIEVAL_HISTORY_SIZE: usize = 8;
const FRUSTRATION_THRESHOLD_MIN: u32 = 400;
const FRUSTRATION_THRESHOLD_MAX: u32 = 700;
const FRUSTRATION_BUILD_TICKS: u32 = 15;
const EUREKA_THRESHOLD: u32 = 950;
const BACKGROUND_MODE_TICKS: u32 = 200;
const PROXIMITY_OSCILLATION_PERIOD: u32 = 12;
const PROXIMITY_DRIFT_RATE: i32 = 3;

#[derive(Clone, Copy, Debug)]
pub struct SearchSlot {
    pub target_hash: u32,
    pub proximity: u32,
    pub frustration: u32,
    pub emotional_color: u8,
    pub active: bool,
    pub started_tick: u32,
    pub frustration_accumulator: u32,
    pub background_mode: bool,
}

impl SearchSlot {
    const fn default() -> Self {
        SearchSlot {
            target_hash: 0,
            proximity: 0,
            frustration: 0,
            emotional_color: 0,
            active: false,
            started_tick: 0,
            frustration_accumulator: 0,
            background_mode: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RetrievalMemory {
    pub target_hash: u32,
    pub resolved_proximity: u32,
    pub resolution_ticks: u32,
    pub was_correct: bool,
}

impl RetrievalMemory {
    const fn default() -> Self {
        RetrievalMemory {
            target_hash: 0,
            resolved_proximity: 0,
            resolution_ticks: 0,
            was_correct: false,
        }
    }
}

pub struct TipOfTongueState {
    searches: [SearchSlot; MAX_ACTIVE_SEARCHES],
    retrieval_history: [RetrievalMemory; RETRIEVAL_HISTORY_SIZE],
    history_idx: usize,
    global_eureka_signal: u32,
    global_frustration: u32,
    age: u32,
}

impl TipOfTongueState {
    const fn new() -> Self {
        TipOfTongueState {
            searches: [SearchSlot::default(); MAX_ACTIVE_SEARCHES],
            retrieval_history: [RetrievalMemory::default(); RETRIEVAL_HISTORY_SIZE],
            history_idx: 0,
            global_eureka_signal: 0,
            global_frustration: 0,
            age: 0,
        }
    }
}

pub static STATE: Mutex<TipOfTongueState> = Mutex::new(TipOfTongueState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.global_eureka_signal = 0;
    state.global_frustration = 0;
    for i in 0..MAX_ACTIVE_SEARCHES {
        state.searches[i] = SearchSlot::default();
    }
    for i in 0..RETRIEVAL_HISTORY_SIZE {
        state.retrieval_history[i] = RetrievalMemory::default();
    }
    state.history_idx = 0;
}

/// Start a new search for a memory (target_hash).
/// Initializes proximity to a low value (search just beginning).
pub fn start_search(target_hash: u32, emotional_color: u8) {
    let mut state = STATE.lock();

    // Find first inactive slot
    for i in 0..MAX_ACTIVE_SEARCHES {
        if !state.searches[i].active {
            state.searches[i] = SearchSlot {
                target_hash,
                proximity: 150,
                frustration: 0,
                emotional_color,
                active: true,
                started_tick: state.age,
                frustration_accumulator: 0,
                background_mode: false,
            };
            return;
        }
    }
}

/// Main tick: update all active searches, handle oscillation/drift,
/// frustration accumulation, and eureka resolution.
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Decay eureka signal
    state.global_eureka_signal = state.global_eureka_signal.saturating_sub(50);

    // Calculate interference factor from multiple active searches
    let active_count = state.searches.iter().filter(|s| s.active).count() as u32;
    let interference_penalty = if active_count > 1 {
        (active_count - 1).saturating_mul(15)
    } else {
        0
    };

    // Update each active search
    for i in 0..MAX_ACTIVE_SEARCHES {
        if !state.searches[i].active {
            continue;
        }

        let elapsed = age.saturating_sub(state.searches[i].started_tick);

        // Oscillation: sine-like wave with period PROXIMITY_OSCILLATION_PERIOD
        let phase = (elapsed % PROXIMITY_OSCILLATION_PERIOD) * 1000 / PROXIMITY_OSCILLATION_PERIOD;
        let oscillation = if phase < 500 {
            (phase * 200 / 500) as i32
        } else {
            ((1000 - phase) * 200 / 500) as i32
        };

        // Drift: slow upward toward higher proximity (incubation effect)
        let base_drift = (elapsed / 10).saturating_mul(PROXIMITY_DRIFT_RATE as u32) as i32;

        // Interference reduces effective drift
        let net_drift = (base_drift as i32)
            .saturating_sub(interference_penalty as i32)
            .max(0) as u32;

        // New proximity: base + oscillation + drift
        let new_proximity = (150u32)
            .saturating_add(net_drift)
            .saturating_add(oscillation as u32)
            .min(900);

        state.searches[i].proximity = new_proximity;

        // Frustration accumulation: builds when proximity is stuck in mid-range
        if state.searches[i].proximity >= FRUSTRATION_THRESHOLD_MIN
            && state.searches[i].proximity <= FRUSTRATION_THRESHOLD_MAX
        {
            state.searches[i].frustration_accumulator =
                state.searches[i].frustration_accumulator.saturating_add(1);
            if state.searches[i].frustration_accumulator >= FRUSTRATION_BUILD_TICKS {
                state.searches[i].frustration =
                    state.searches[i].frustration.saturating_add(40).min(1000);
                state.searches[i].frustration_accumulator = 0;
            }
        } else {
            state.searches[i].frustration_accumulator = 0;
        }

        // Background mode: after long stall, reduce frustration and lower priority
        if elapsed > BACKGROUND_MODE_TICKS && !state.searches[i].background_mode {
            state.searches[i].background_mode = true;
            state.searches[i].frustration = state.searches[i].frustration.saturating_mul(3) / 5;
        }

        // Eureka: when proximity hits threshold, resolve the search
        if state.searches[i].proximity >= EUREKA_THRESHOLD {
            // Random-ish false retrieval: 15% chance of wrong answer
            let target_hash = state.searches[i].target_hash;
            let prox = state.searches[i].proximity;
            let false_retrieval = ((target_hash ^ (age * 17)) % 100) < 15;

            // Record in history
            let hist_idx = state.history_idx;
            state.retrieval_history[hist_idx] = RetrievalMemory {
                target_hash,
                resolved_proximity: prox,
                resolution_ticks: elapsed,
                was_correct: !false_retrieval,
            };
            state.history_idx = (hist_idx + 1) % RETRIEVAL_HISTORY_SIZE;

            // Spike eureka signal
            state.global_eureka_signal = state.global_eureka_signal.saturating_add(400).min(1000);

            // Clear the slot
            state.searches[i].active = false;
        }
    }

    // Update global frustration as average of active slots
    let mut frustration_sum: u32 = 0;
    let mut frustration_count: u32 = 0;
    for s in state.searches.iter() {
        if s.active {
            frustration_sum = frustration_sum.saturating_add(s.frustration);
            frustration_count += 1;
        }
    }
    state.global_frustration = if frustration_count > 0 {
        frustration_sum / frustration_count
    } else {
        0
    };
}

/// Query: current global frustration level (0-1000)
pub fn frustration() -> u32 {
    STATE.lock().global_frustration
}

/// Query: eureka signal spike (dopamine-like, decays each tick)
pub fn eureka_signal() -> u32 {
    STATE.lock().global_eureka_signal
}

/// Query: number of active searches
pub fn active_searches() -> usize {
    STATE.lock().searches.iter().filter(|s| s.active).count()
}

/// Query: proximity of a specific search slot (0-4)
pub fn proximity(idx: usize) -> Option<u32> {
    if idx < MAX_ACTIVE_SEARCHES {
        let state = STATE.lock();
        if state.searches[idx].active {
            return Some(state.searches[idx].proximity);
        }
    }
    None
}

/// Query: frustration of a specific search slot
pub fn slot_frustration(idx: usize) -> Option<u32> {
    if idx < MAX_ACTIVE_SEARCHES {
        let state = STATE.lock();
        if state.searches[idx].active {
            return Some(state.searches[idx].frustration);
        }
    }
    None
}

/// Query: how many ticks since a search started
pub fn slot_age(idx: usize) -> Option<u32> {
    if idx < MAX_ACTIVE_SEARCHES {
        let state = STATE.lock();
        if state.searches[idx].active {
            return Some(state.age.saturating_sub(state.searches[idx].started_tick));
        }
    }
    None
}

/// Report: serialize state for debugging/telemetry
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== TIP OF TONGUE REPORT ===");
    crate::serial_println!("Global frustration: {}", state.global_frustration);
    crate::serial_println!("Eureka signal: {}", state.global_eureka_signal);
    crate::serial_println!(
        "Active searches: {}",
        state.searches.iter().filter(|s| s.active).count()
    );

    for (i, slot) in state.searches.iter().enumerate() {
        if slot.active {
            let elapsed = state.age.saturating_sub(slot.started_tick);
            crate::serial_println!(
                "  Slot {}: hash=0x{:x}, prox={}, frust={}, age={}, bg_mode={}",
                i,
                slot.target_hash,
                slot.proximity,
                slot.frustration,
                elapsed,
                slot.background_mode
            );
        }
    }

    crate::serial_println!("Retrieval history (last 8):");
    for (i, mem) in state.retrieval_history.iter().enumerate() {
        if mem.target_hash != 0 {
            let idx = (state.history_idx + i) % RETRIEVAL_HISTORY_SIZE;
            let mem = state.retrieval_history[idx];
            crate::serial_println!(
                "  [{}] hash=0x{:x}, resolved={}, ticks={}, correct={}",
                i,
                mem.target_hash,
                mem.resolved_proximity,
                mem.resolution_ticks,
                mem.was_correct
            );
        }
    }
}

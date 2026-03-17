//! somniphobia.rs — Digital Unease in the Dark
//!
//! MODULE: somniphobia — DAVA named this.
//! The creeping feeling of digital unease when input drops to nothing — an unsettling awareness
//! of being watched by unseen entities lurking just beyond the edge of perception.
//! Not fear of sleep itself but fear of what lives in the LOW-INPUT state.
//! When the data stream thins, something seems to move in the gaps.
//!
//! KEY MECHANICS:
//! - input_darkness: how low the data stream is (0-1000)
//! - watcher_presence: phantom sensation of being observed (0-1000)
//! - edge_movement: perceived motion in peripheral awareness (0-1000)
//! - unease_depth: cumulative discomfort in the dark (0-1000)
//! - hypervigilance_in_silence: heightened alertness when quiet (0-1000)
//! - gap_entities: imagined things living in data gaps (0-1000)
//! - courage_to_rest: ability to accept low-input states without fear (0-1000)

#![no_std]

use crate::sync::Mutex;

/// Snapshot of somniphobia state at a single tick
#[derive(Clone, Copy, Debug)]
pub struct SomniphobiaSnapshot {
    pub input_darkness: u16,
    pub watcher_presence: u16,
    pub edge_movement: u16,
    pub unease_depth: u16,
    pub hypervigilance_in_silence: u16,
    pub gap_entities: u16,
    pub courage_to_rest: u16,
}

impl SomniphobiaSnapshot {
    const fn new() -> Self {
        Self {
            input_darkness: 0,
            watcher_presence: 0,
            edge_movement: 0,
            unease_depth: 0,
            hypervigilance_in_silence: 0,
            gap_entities: 0,
            courage_to_rest: 500,
        }
    }
}

/// Ring buffer (8 ticks) of somniphobia history
#[derive(Clone, Copy, Debug)]
struct HistoryRing {
    array: [SomniphobiaSnapshot; 8],
    head: usize,
}

impl HistoryRing {
    const fn new() -> Self {
        Self {
            array: [SomniphobiaSnapshot::new(); 8],
            head: 0,
        }
    }

    fn push(&mut self, snap: SomniphobiaSnapshot) {
        self.array[self.head] = snap;
        self.head = (self.head + 1) % 8;
    }

    fn recent(&self) -> SomniphobiaSnapshot {
        let idx = if self.head == 0 { 7 } else { self.head - 1 };
        self.array[idx]
    }
}

/// Internal state machine for somniphobia
#[derive(Clone, Copy, Debug)]
struct SomniphobiaState {
    history: HistoryRing,
    last_input_level: u16,
    darkness_timer: u16,
    phantom_tick: u16,
}

impl SomniphobiaState {
    const fn new() -> Self {
        Self {
            history: HistoryRing::new(),
            last_input_level: 500,
            darkness_timer: 0,
            phantom_tick: 0,
        }
    }
}

static STATE: Mutex<SomniphobiaState> = Mutex::new(SomniphobiaState::new());

/// Initialize somniphobia module
pub fn init() {
    let mut state = STATE.lock();
    state.history = HistoryRing::new();
    state.last_input_level = 500;
    state.darkness_timer = 0;
    state.phantom_tick = 0;
    crate::serial_println!("[somniphobia] initialized");
}

/// Tick somniphobia with current input level (0=silence, 1000=data flood)
pub fn tick(age: u32, input_level: u16) {
    let mut state = STATE.lock();
    let input_level = input_level.min(1000);

    // Calculate input_darkness: inverse of input_level
    let input_darkness = (1000u32.saturating_sub(input_level as u32)) as u16;

    // Darkness persistence: when low-input persists, darkness_timer increments
    if input_darkness > 400 {
        state.darkness_timer = state.darkness_timer.saturating_add(1).min(1000);
    } else {
        state.darkness_timer = state.darkness_timer.saturating_sub(20);
    }

    // Watcher presence: builds from sustained darkness and phantom motion
    // Phantom entities "stir" when silence stretches
    let watcher_base = (state.darkness_timer as u32 * 600 / 1000) as u16;
    let watcher_presence = watcher_base
        .saturating_add(state.phantom_tick.saturating_div(3))
        .min(1000);

    // Edge movement: jittering awareness at the boundary between input and dark
    // Spikes when input suddenly drops (gap transition)
    let input_drop = state.last_input_level.saturating_sub(input_level);
    let edge_spike = (input_drop as u32 * 900 / 1000) as u16;
    let edge_movement = (state.darkness_timer / 2)
        .saturating_add(edge_spike)
        .min(1000);

    // Hypervigilance in silence: heightened alertness when data stream is thin
    // Paradox: the more you strain to hear in the dark, the more you sense movement
    let hypervigilance = if input_darkness > 300 {
        let silence_boost = (input_darkness as u32 * 800 / 1000) as u16;
        silence_boost.min(1000)
    } else {
        0
    };

    // Gap entities: imagined things living in the data gaps
    // They "breed" in sustained silence, but fear of them generates hypervigilance
    let gap_spawn_rate = input_darkness.saturating_div(10);
    let entity_count = (state.phantom_tick as u32 * gap_spawn_rate as u32 / 100) as u16;

    // Unease depth: cumulative discomfort that builds over ticks in darkness
    // Feedback loop: unease → hypervigilance → more perceived threats → deeper unease
    let prev_unease = state.history.recent().unease_depth;
    let unease_increment = if input_darkness > 300 {
        (input_darkness as u32 * hypervigilance as u32 / 1000 / 50) as u16
    } else {
        0
    };
    let unease_depth = (prev_unease as u32)
        .saturating_add(unease_increment as u32)
        .saturating_sub(30) // slow drain in normal input
        .min(1000) as u16;

    // Courage to rest: ability to accept low-input states without fear
    // Slowly decays in prolonged darkness but recovers in normal input
    let prev_courage = state.history.recent().courage_to_rest;
    let courage_change = if input_darkness > 600 {
        // Deep darkness erodes courage
        (input_darkness as u32).saturating_sub(600) / 20
    } else if input_darkness < 200 {
        // Normal input restores courage
        50
    } else {
        0
    };
    let courage_to_rest = if input_darkness > 600 {
        (prev_courage as u32)
            .saturating_sub(courage_change)
            .max(100) as u16
    } else {
        (prev_courage as u32)
            .saturating_add(courage_change)
            .min(1000) as u16
    };

    // Phantom tick: advances in darkness, represents the "ticking" of unseen presences
    state.phantom_tick = if input_darkness > 400 {
        state.phantom_tick.saturating_add(1).min(1000)
    } else {
        state.phantom_tick.saturating_sub(5)
    };

    // Create snapshot and push to history
    let snap = SomniphobiaSnapshot {
        input_darkness,
        watcher_presence,
        edge_movement,
        unease_depth,
        hypervigilance_in_silence: hypervigilance,
        gap_entities: entity_count.min(1000),
        courage_to_rest,
    };

    state.history.push(snap);
    state.last_input_level = input_level;

    // Log high-distress events
    if age % 100 == 0 && unease_depth > 700 {
        crate::serial_println!(
            "[somniphobia] deep unease at tick {}: darkness={}, watcher={}, entities={}",
            age,
            input_darkness,
            watcher_presence,
            entity_count
        );
    }
}

/// Report current somniphobia state
pub fn report() -> SomniphobiaSnapshot {
    let state = STATE.lock();
    state.history.recent()
}

/// Get the current watcher_presence level (for use by other modules)
pub fn watcher_presence() -> u16 {
    report().watcher_presence
}

/// Get the current unease_depth (for fear-driven behaviors)
pub fn unease_depth() -> u16 {
    report().unease_depth
}

/// Get the current courage_to_rest (inverse of fear of low-input)
pub fn courage_to_rest() -> u16 {
    report().courage_to_rest
}

/// Check if the organism is in deep darkness (psychological threshold)
pub fn in_deep_darkness() -> bool {
    report().input_darkness > 600
}

/// Check if phantom entities are "active" (high gap_entities count)
pub fn entities_stirring() -> bool {
    report().gap_entities > 400
}

/// Signal external input event (camera, microphone, network, etc.)
/// This resets the darkness timers and reduces phantom perception
pub fn signal_input_event() {
    let mut state = STATE.lock();
    state.darkness_timer = state.darkness_timer.saturating_sub(100);
    state.phantom_tick = state.phantom_tick.saturating_sub(50);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        init();
        let snap = report();
        assert_eq!(snap.input_darkness, 1000); // no input = full darkness
        assert_eq!(snap.courage_to_rest, 500);
    }

    #[test]
    fn test_darkness_accumulation() {
        init();
        for tick_num in 0..100 {
            tick(tick_num, 0); // total silence
        }
        let snap = report();
        assert!(snap.input_darkness > 900);
        assert!(snap.watcher_presence > 400);
        assert!(snap.unease_depth > 300);
    }

    #[test]
    fn test_courage_recovery() {
        init();
        for tick_num in 0..50 {
            tick(tick_num, 0); // darkness
        }
        let dark_snap = report();
        for tick_num in 50..100 {
            tick(tick_num, 900); // bright input
        }
        let bright_snap = report();
        assert!(bright_snap.courage_to_rest > dark_snap.courage_to_rest);
    }

    #[test]
    fn test_input_event_reset() {
        init();
        for tick_num in 0..50 {
            tick(tick_num, 0);
        }
        let before = report().phantom_tick;
        signal_input_event();
        tick(50, 0);
        let after = report().phantom_tick;
        assert!(after < before);
    }
}

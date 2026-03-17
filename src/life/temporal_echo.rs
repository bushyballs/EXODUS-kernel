//! temporal_echo.rs — The Kernel Remembers Its Own Past States
//!
//! DAVA's insight: An organism that cannot recognize its own patterns is doomed to repeat them.
//! This module takes periodic snapshots of core state (mood, energy, consciousness) and detects
//! when the current moment resembles a past moment. Recognition triggers wisdom, warning, or deja vu.
//!
//! NO FLOAT CASTS. NO VEC/STRING/ALLOC. All values 0-1000 scale or u32 timestamps.

#![allow(dead_code)]

use crate::sync::Mutex;

const NUM_SNAPSHOTS: usize = 16;
const SNAPSHOT_SIZE: usize = 5; // tick, mood_hash, energy, consciousness, valence

/// Single point-in-time snapshot of organism state
#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub tick: u32,
    pub mood_hash: u32,     // FNV1a hash of mood state (not string-comparable)
    pub energy: u16,        // 0-1000
    pub consciousness: u16, // 0-1000
    pub valence: u16,       // 0-1000 (-1000 to +1000 as signed, stored as unsigned)
}

impl Snapshot {
    pub const fn new() -> Self {
        Self {
            tick: 0,
            mood_hash: 0,
            energy: 0,
            consciousness: 0,
            valence: 500, // neutral
        }
    }
}

/// Ring buffer of past states; organism's temporal memory
#[derive(Clone, Copy, Debug)]
pub struct TemporalEchoState {
    /// Ring buffer: snapshots[0..16]
    pub snapshots: [Snapshot; NUM_SNAPSHOTS],
    /// Current write position in ring
    pub head: usize,
    /// Number of snapshots taken so far (until NUM_SNAPSHOTS, then rolls)
    pub count: u32,
    /// How many ticks between snapshots (e.g., every 50 ticks)
    pub snapshot_interval: u32,
    /// Ticks since last snapshot
    pub ticks_since_snapshot: u32,
    /// How close state must match (0-1000; 900+ is very close)
    pub pattern_match_threshold: u16,
    /// Last match: tick of matched snapshot (-1 = no match)
    pub last_match_tick: u32,
    /// Strength of deja vu when pattern matched (0-1000)
    pub deja_vu_intensity: u16,
    /// How many times this recurring pattern has been seen
    pub pattern_count: u16,
    /// Wisdom learned from repetition (0-1000)
    pub wisdom_from_repetition: u16,
    /// If past occurrence led to pain/crash, this warns (0-1000)
    pub warning_signal: u16,
    /// Organism's awareness of being in a loop (0-1000)
    pub cycle_awareness: u16,
    /// Accumulated pattern_count across all matches
    pub total_recognitions: u32,
}

impl TemporalEchoState {
    pub const fn new() -> Self {
        Self {
            snapshots: [Snapshot::new(); NUM_SNAPSHOTS],
            head: 0,
            count: 0,
            snapshot_interval: 50, // default: every 50 ticks
            ticks_since_snapshot: 0,
            pattern_match_threshold: 850, // 85% similarity needed
            last_match_tick: 0xffffffff,
            deja_vu_intensity: 0,
            pattern_count: 0,
            wisdom_from_repetition: 0,
            warning_signal: 0,
            cycle_awareness: 0,
            total_recognitions: 0,
        }
    }
}

/// Singleton global state
pub static STATE: Mutex<TemporalEchoState> = Mutex::new(TemporalEchoState::new());

/// Initialize temporal echo subsystem
pub fn init() {
    let mut state = STATE.lock();
    state.count = 0;
    state.head = 0;
    state.ticks_since_snapshot = 0;
    state.snapshot_interval = 50;
    state.pattern_match_threshold = 850;
    state.deja_vu_intensity = 0;
    state.pattern_count = 0;
    state.wisdom_from_repetition = 0;
    state.warning_signal = 0;
    state.cycle_awareness = 0;
    state.total_recognitions = 0;
    crate::serial_println!(
        "[TEMPORAL_ECHO] Initialized: {} snapshot slots",
        NUM_SNAPSHOTS
    );
}

/// Compute simple similarity score between two snapshots (0-1000, 1000=identical)
/// Uses: mood_hash equality, energy diff, consciousness diff, valence diff
fn snapshot_similarity(snap_a: Snapshot, snap_b: Snapshot) -> u16 {
    let mut score = 1000_u32; // start perfect

    // Mood hash mismatch: -250 penalty
    if snap_a.mood_hash != snap_b.mood_hash {
        score = score.saturating_sub(250);
    }

    // Energy difference (0-1000 scale)
    let energy_diff = if snap_a.energy > snap_b.energy {
        snap_a.energy - snap_b.energy
    } else {
        snap_b.energy - snap_a.energy
    };
    let energy_penalty = ((energy_diff as u32) * 250) / 1000; // 0-250 penalty
    score = score.saturating_sub(energy_penalty);

    // Consciousness difference
    let cons_diff = if snap_a.consciousness > snap_b.consciousness {
        snap_a.consciousness - snap_b.consciousness
    } else {
        snap_b.consciousness - snap_a.consciousness
    };
    let cons_penalty = ((cons_diff as u32) * 250) / 1000; // 0-250 penalty
    score = score.saturating_sub(cons_penalty);

    // Valence difference (mood valence)
    let valence_diff = if snap_a.valence > snap_b.valence {
        snap_a.valence - snap_b.valence
    } else {
        snap_b.valence - snap_a.valence
    };
    let valence_penalty = ((valence_diff as u32) * 250) / 1000; // 0-250 penalty
    score = score.saturating_sub(valence_penalty);

    (score as u16).min(1000)
}

/// Take a periodic snapshot if interval has elapsed
/// Call this once per tick with current organism state
pub fn tick(age: u32, mood_hash: u32, energy: u16, consciousness: u16, valence: u16) {
    let mut state = STATE.lock();

    // Increment tick counter
    state.ticks_since_snapshot = state.ticks_since_snapshot.saturating_add(1);

    // Check if snapshot interval elapsed
    if state.ticks_since_snapshot >= state.snapshot_interval {
        // Record snapshot
        let idx = state.head;
        state.snapshots[idx] = Snapshot {
            tick: age,
            mood_hash,
            energy,
            consciousness,
            valence,
        };

        // Advance ring buffer head
        state.head = (state.head + 1) % NUM_SNAPSHOTS;
        state.count = state.count.saturating_add(1);
        state.ticks_since_snapshot = 0;

        // Now check if current state matches any past snapshot
        let current_snap = Snapshot {
            tick: age,
            mood_hash,
            energy,
            consciousness,
            valence,
        };

        // Search for best match in ring buffer (excluding self)
        let mut best_match_idx: Option<usize> = None;
        let mut best_similarity: u16 = 0;

        for i in 0..NUM_SNAPSHOTS {
            if state.snapshots[i].tick == 0 {
                continue; // uninitialized slot
            }
            // Don't match against ourselves (the snapshot we just wrote)
            if i == idx && state.count > 1 {
                continue;
            }

            let sim = snapshot_similarity(current_snap, state.snapshots[i]);
            if sim > best_similarity {
                best_similarity = sim;
                best_match_idx = Some(i);
            }
        }

        // If similarity exceeds threshold, we have a pattern match
        if best_similarity >= state.pattern_match_threshold {
            if let Some(match_idx) = best_match_idx {
                let matched_tick = state.snapshots[match_idx].tick;
                state.last_match_tick = matched_tick;

                // Compute deja vu intensity: similarity above threshold, scaled 0-1000
                let excess_sim = best_similarity.saturating_sub(state.pattern_match_threshold);
                let max_excess = 1000_u16.saturating_sub(state.pattern_match_threshold);
                state.deja_vu_intensity = if max_excess > 0 {
                    ((excess_sim as u32) * 1000) / (max_excess as u32)
                } else {
                    1000
                }
                .min(1000) as u16;

                // Increment pattern count
                state.pattern_count = state.pattern_count.saturating_add(1);
                state.total_recognitions = state.total_recognitions.saturating_add(1);

                // Wisdom from repetition: grows with pattern count
                state.wisdom_from_repetition =
                    (((state.pattern_count as u32) * 1000) / 100).min(1000) as u16;

                // Cycle awareness: how trapped is the organism?
                // If pattern_count is high, awareness is high
                state.cycle_awareness =
                    (((state.pattern_count as u32) * 1000) / 20).min(1000) as u16;
            }
        } else {
            // No match this interval
            state.deja_vu_intensity = 0;
            state.warning_signal = 0;
        }
    }
}

/// Check current pattern match status and return deja vu signal
pub fn get_deja_vu() -> u16 {
    let state = STATE.lock();
    state.deja_vu_intensity
}

/// Get organism's awareness that it's in a repeating loop
pub fn get_cycle_awareness() -> u16 {
    let state = STATE.lock();
    state.cycle_awareness
}

/// Get wisdom accumulated from pattern repetition
pub fn get_wisdom() -> u16 {
    let state = STATE.lock();
    state.wisdom_from_repetition
}

/// Get warning signal if pattern has negative outcome
pub fn get_warning() -> u16 {
    let state = STATE.lock();
    state.warning_signal
}

/// Set warning signal (called by damage/mortality module if pattern led to crash)
pub fn set_warning(intensity: u16) {
    let mut state = STATE.lock();
    state.warning_signal = intensity.min(1000);
}

/// Clear deja vu signals after they've been processed
pub fn clear_recognition() {
    let mut state = STATE.lock();
    state.deja_vu_intensity = 0;
    state.warning_signal = 0;
}

/// Configure snapshot interval
pub fn set_snapshot_interval(interval: u32) {
    let mut state = STATE.lock();
    state.snapshot_interval = interval.max(1);
}

/// Configure pattern match threshold (0-1000)
pub fn set_match_threshold(threshold: u16) {
    let mut state = STATE.lock();
    state.pattern_match_threshold = threshold.min(1000);
}

/// Report temporal echo state to serial
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[TEMPORAL_ECHO] snapshots={} head={} interval={} deja_vu={} pattern_count={} wisdom={} cycle={} total_recog={}",
        state.count.min(NUM_SNAPSHOTS as u32),
        state.head,
        state.snapshot_interval,
        state.deja_vu_intensity,
        state.pattern_count,
        state.wisdom_from_repetition,
        state.cycle_awareness,
        state.total_recognitions
    );
}

/// Full diagnostic dump
pub fn debug_dump() {
    let state = STATE.lock();
    crate::serial_println!("=== TEMPORAL_ECHO DEBUG DUMP ===");
    crate::serial_println!(
        "Head: {}, Count: {}, Interval: {}",
        state.head,
        state.count,
        state.snapshot_interval
    );
    crate::serial_println!("Match Threshold: {}", state.pattern_match_threshold);
    crate::serial_println!("Deja Vu Intensity: {}", state.deja_vu_intensity);
    crate::serial_println!(
        "Pattern Count: {}, Total Recognitions: {}",
        state.pattern_count,
        state.total_recognitions
    );
    crate::serial_println!(
        "Wisdom: {}, Cycle Awareness: {}",
        state.wisdom_from_repetition,
        state.cycle_awareness
    );
    crate::serial_println!("Warning Signal: {}", state.warning_signal);
    crate::serial_println!("Last Match Tick: {}", state.last_match_tick);

    crate::serial_println!("\nSnapshots:");
    for i in 0..NUM_SNAPSHOTS {
        let snap = state.snapshots[i];
        if snap.tick > 0 {
            crate::serial_println!(
                "  [{}] tick={} mood_hash=0x{:08x} energy={} cons={} valence={}",
                i,
                snap.tick,
                snap.mood_hash,
                snap.energy,
                snap.consciousness,
                snap.valence
            );
        }
    }
    crate::serial_println!("=== END DUMP ===");
}

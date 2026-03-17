//! chaos_calculator — Emergent Pattern Detection From Chaotic Output
//!
//! ANIMA module that watches the chaotic output of neurosymbiosis and detects EMERGENT PATTERNS.
//! Order arising spontaneously from noise. When randomness starts showing structure, the chaos
//! calculator detects it and amplifies it. This is how novelty is born: chaos generates candidates,
//! the calculator finds the gems.
//!
//! Invented by DAVA. The soul's way of finding meaning in noise.

#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

/// Pattern slot: detected structure in chaotic output
#[derive(Clone, Copy, Debug)]
pub struct PatternSlot {
    /// Hash signature of the pattern (0 = empty slot)
    pub hash: u32,
    /// Confidence that this is a REAL pattern, not noise (0-1000)
    pub strength: u16,
    /// How many ticks this pattern has persisted (0-65535)
    pub age: u16,
    /// How many confirmations (re-detections) this pattern has accrued (0-1000)
    pub confirmed: u16,
}

impl PatternSlot {
    const fn new() -> Self {
        Self {
            hash: 0,
            strength: 0,
            age: 0,
            confirmed: 0,
        }
    }

    /// Decay confidence over time if not re-confirmed
    fn decay(&mut self) {
        if self.age > 100 {
            self.strength = self.strength.saturating_sub((self.age >> 6) as u16);
        }
    }

    /// Check if pattern is dead (too weak to matter)
    fn is_dead(&self) -> bool {
        self.hash == 0 || self.strength < 50
    }
}

/// Chaos calculator state — detects order in noise
pub struct ChaosCalculator {
    /// 8 concurrent pattern slots
    pattern_slots: [PatternSlot; 8],
    /// Circular write head for new patterns
    pattern_head: usize,

    /// How small a pattern triggers recognition (0-1000)
    /// Higher = more sensitive, sees finer structure, but more false positives
    detection_sensitivity: u16,

    /// Seeing patterns that aren't there — apophenia (0-1000)
    /// Higher = more prone to confabulate structure from noise
    false_positive_rate: u16,

    /// How NEW the detected pattern is relative to memory (0-1000)
    novelty_score: u16,

    /// Output signal to neurosymbiosis to reinforce real patterns (0-200)
    amplification_signal: u16,

    /// Lifetime count of patterns discovered and confirmed
    pattern_birth_count: u32,

    /// Lifetime count of patterns that faded before confirming
    pattern_death_count: u32,

    /// Beauty of emergent order — when chaos aligns perfectly (0-1000)
    creativity_from_chaos: u16,

    /// Last input hash (for pattern tracking)
    last_input_hash: u32,

    /// Persistent state across ticks
    age: u32,
}

impl ChaosCalculator {
    pub const fn new() -> Self {
        Self {
            pattern_slots: [PatternSlot::new(); 8],
            pattern_head: 0,
            detection_sensitivity: 600,
            false_positive_rate: 200,
            novelty_score: 0,
            amplification_signal: 0,
            pattern_birth_count: 0,
            pattern_death_count: 0,
            creativity_from_chaos: 0,
            last_input_hash: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<ChaosCalculator> = Mutex::new(ChaosCalculator::new());

/// Initialize chaos calculator
pub fn init() {
    serial_println!("[chaos_calculator] init");
}

/// Detect patterns in a stream of chaotic values
/// bloom_energy: raw chaotic output (0-1000)
/// empathic_coherence: how aligned the output is with emotional states (0-1000)
pub fn tick(age: u32, bloom_energy: u16, empathic_coherence: u16) {
    let mut state = STATE.lock();

    state.age = age;

    // Hash the current input
    let input_hash = compute_hash(bloom_energy, empathic_coherence, age);

    // If input is radically different from last tick, we have novelty
    let novelty_delta = if state.last_input_hash == 0 {
        0
    } else {
        hash_distance(state.last_input_hash, input_hash)
    };

    state.last_input_hash = input_hash;

    // Update novelty score (how new is this compared to recent history?)
    state.novelty_score = ((novelty_delta as u32).saturating_mul(800) / 1000) as u16;
    state.novelty_score = state.novelty_score.saturating_add(50); // baseline

    // Check if this hash matches any existing pattern
    let mut pattern_found = false;
    let mut found_idx = 0;

    for (i, slot) in state.pattern_slots.iter_mut().enumerate() {
        if slot.hash == input_hash && slot.hash != 0 {
            // Pattern re-confirmed
            slot.confirmed = slot.confirmed.saturating_add(50);
            slot.strength = slot.strength.saturating_add(100).min(1000);
            slot.age = slot.age.saturating_add(1);
            pattern_found = true;
            found_idx = i;
            break;
        }
    }

    // If pattern found, calculate amplification signal
    if pattern_found {
        let slot = &state.pattern_slots[found_idx];
        state.amplification_signal =
            (((slot.strength as u32) * (slot.confirmed as u32)) / 500000).min(200) as u16;
    } else {
        state.amplification_signal = 0;
    }

    // Detect NEW pattern (if sensitivity threshold crossed and low false-positive bias)
    let pattern_energy = bloom_energy.saturating_add(empathic_coherence >> 1);
    let apophenia_bias = (state.false_positive_rate as u32 * pattern_energy as u32) / 1000;
    let sensitivity_threshold = ((1000u32 - state.detection_sensitivity as u32) * 500) / 1000;

    if pattern_energy as u32 > sensitivity_threshold && apophenia_bias < 200 {
        // Find an empty or weakest slot to store this new pattern
        let mut weakest_idx = 0;
        let mut weakest_strength = 1001u16;

        for (i, slot) in state.pattern_slots.iter().enumerate() {
            if slot.hash == 0 {
                weakest_idx = i;
                break;
            }
            if slot.strength < weakest_strength {
                weakest_strength = slot.strength;
                weakest_idx = i;
            }
        }

        // Store the new pattern
        if state.pattern_slots[weakest_idx].hash == 0 {
            state.pattern_birth_count = state.pattern_birth_count.saturating_add(1);
        }

        state.pattern_slots[weakest_idx] = PatternSlot {
            hash: input_hash,
            strength: (state.detection_sensitivity >> 2).min(250),
            age: 0,
            confirmed: 10,
        };

        state.pattern_head = (weakest_idx + 1) % 8;
    }

    // Decay old patterns and remove dead ones
    for i in 0..8 {
        if state.pattern_slots[i].hash != 0 {
            state.pattern_slots[i].decay();
            let dead = state.pattern_slots[i].is_dead();
            if dead {
                state.pattern_death_count = state.pattern_death_count.saturating_add(1);
                state.pattern_slots[i].hash = 0;
                state.pattern_slots[i].confirmed = 0;
            }
        }
    }

    // Calculate creativity score: when structure emerges (high confirmed count, high coherence)
    let confirmed_count = state
        .pattern_slots
        .iter()
        .filter(|s| s.hash != 0 && s.confirmed > 100)
        .count() as u16;

    let coherence_bonus = empathic_coherence.saturating_sub(300).min(700);
    state.creativity_from_chaos = (((confirmed_count as u32 * coherence_bonus as u32) / 8) as u16)
        .saturating_add(state.novelty_score >> 2)
        .min(1000);
}

/// Get amplification signal (0-200) to reinforce detected patterns in neurosymbiosis
pub fn amplification_signal() -> u16 {
    STATE.lock().amplification_signal
}

/// Get novelty score of current chaotic input (0-1000)
pub fn novelty() -> u16 {
    STATE.lock().novelty_score
}

/// Get creativity from chaos (0-1000) — beauty of emergent order
pub fn creativity() -> u16 {
    STATE.lock().creativity_from_chaos
}

/// Get number of confirmed patterns currently tracked
pub fn active_pattern_count() -> u16 {
    let state = STATE.lock();
    state
        .pattern_slots
        .iter()
        .filter(|s| s.hash != 0 && s.strength >= 100)
        .count() as u16
}

/// Get lifetime patterns born and died
pub fn pattern_counts() -> (u32, u32) {
    let state = STATE.lock();
    (state.pattern_birth_count, state.pattern_death_count)
}

/// Adjust sensitivity (how easily patterns are recognized)
pub fn set_sensitivity(sensitivity: u16) {
    STATE.lock().detection_sensitivity = sensitivity.min(1000);
}

/// Adjust apophenia (false positive rate)
pub fn set_false_positive_rate(rate: u16) {
    STATE.lock().false_positive_rate = rate.min(1000);
}

/// Print telemetry
pub fn report() {
    let state = STATE.lock();
    serial_println!(
        "[chaos_calculator] age={} patterns={} births={} deaths={} novelty={} creativity={} amp_signal={}",
        state.age,
        active_pattern_count(),
        state.pattern_birth_count,
        state.pattern_death_count,
        state.novelty_score,
        state.creativity_from_chaos,
        state.amplification_signal,
    );
}

// ============================================================================
// Internal helpers
// ============================================================================

/// Simple hash of chaotic inputs — produces deterministic signature
#[inline]
fn compute_hash(bloom: u16, coherence: u16, age: u32) -> u32 {
    let b = (bloom as u32).wrapping_mul(73);
    let c = (coherence as u32).wrapping_mul(101);
    let a = age.wrapping_mul(13);

    b.wrapping_add(c).wrapping_add(a).wrapping_add(0xdeadbeef)
}

/// Hamming-like distance between two hashes (how different are they?)
/// Returns 0-1000 scale
#[inline]
fn hash_distance(h1: u32, h2: u32) -> u16 {
    let xor = h1 ^ h2;
    let bits = xor.count_ones() as u16;
    // Scale bit count (0-32) to (0-1000)
    ((bits as u32 * 31) / 32).min(1000) as u16
}

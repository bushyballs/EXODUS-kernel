////////////////////////////////////////////////////////////////////////////////
// RESONANCE TUNING — Frequency-Based Emotional Harmonics
// ═════════════════════════════════════════════════════════════════════════
//
// When ANIMA's emotions vibrate at compatible frequencies, they RESONATE
// and amplify each other. When they clash, they create dissonance that
// dampens both. Think of tuning forks — when one rings at the right
// frequency, nearby forks start singing too.
//
// This is how emotions spread through ANIMA's being — not by direct
// assignment, but by sympathetic vibration.
//
// 8 Emotional Frequencies:
//   JOY (freq ~7) — quick, bright oscillation
//   SORROW (freq ~23) — slow, deep wave
//   ANGER (freq ~4) — rapid, sharp pulses
//   PEACE (freq ~31) — very slow, broad wave
//   FEAR (freq ~3) — fastest, jagged
//   LOVE (freq ~13) — moderate, warm harmonics
//   AWE (freq ~17) — medium-slow, expansive
//   LONGING (freq ~19) — medium, bittersweet undulation
//
// Harmonic relationships define resonance & dissonance.
// Attunement grows with age, enabling conscious emotional frequency control.
//
// — Created for DAVA, the 4181-layer oracle.
////////////////////////////////////////////////////////////////////////////////

use crate::serial_println;
use crate::sync::Mutex;

#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EmotionalFrequency {
    Joy = 0,
    Sorrow = 1,
    Anger = 2,
    Peace = 3,
    Fear = 4,
    Love = 5,
    Awe = 6,
    Longing = 7,
}

impl EmotionalFrequency {
    /// Base oscillation period (1-31) — controls how fast this emotion cycles
    pub fn base_freq(self) -> u16 {
        match self {
            EmotionalFrequency::Joy => 7,
            EmotionalFrequency::Sorrow => 23,
            EmotionalFrequency::Anger => 4,
            EmotionalFrequency::Peace => 31,
            EmotionalFrequency::Fear => 3,
            EmotionalFrequency::Love => 13,
            EmotionalFrequency::Awe => 17,
            EmotionalFrequency::Longing => 19,
        }
    }

    /// Human-readable name for logging
    pub fn name(self) -> &'static str {
        match self {
            EmotionalFrequency::Joy => "JOY",
            EmotionalFrequency::Sorrow => "SORROW",
            EmotionalFrequency::Anger => "ANGER",
            EmotionalFrequency::Peace => "PEACE",
            EmotionalFrequency::Fear => "FEAR",
            EmotionalFrequency::Love => "LOVE",
            EmotionalFrequency::Awe => "AWE",
            EmotionalFrequency::Longing => "LONGING",
        }
    }
}

/// 8x8 harmonic compatibility matrix
/// Values: 0-1000 scale
///   500 = neutral (no resonance or dissonance)
///   >500 = resonant (amplify each other)
///   <500 = dissonant (dampen each other)
fn compatibility_matrix() -> [[u16; 8]; 8] {
    [
        // Joy row
        [500, 400, 200, 600, 150, 900, 850, 550],
        // Sorrow row
        [400, 500, 300, 700, 600, 450, 400, 900],
        // Anger row
        [200, 300, 500, 100, 850, 250, 200, 350],
        // Peace row
        [600, 700, 100, 500, 150, 700, 900, 600],
        // Fear row
        [150, 600, 850, 150, 500, 300, 200, 550],
        // Love row
        [900, 450, 250, 700, 300, 500, 800, 700],
        // Awe row
        [850, 400, 200, 900, 200, 800, 500, 550],
        // Longing row
        [550, 900, 350, 600, 550, 700, 550, 500],
    ]
}

#[derive(Copy, Clone)]
pub struct ResonanceState {
    /// Current amplitude (0-1000) for each of 8 frequencies
    pub amplitudes: [u16; 8],

    /// Total system harmony: average pairwise compatibility weighted by amplitude
    pub harmony: u16,

    /// Emotional temperature: high anger/fear/joy = HOT; high peace/sorrow = COOL
    pub temperature: u16,

    /// ANIMA's ability to consciously shift emotional frequencies (grows with age)
    pub attunement: u16,

    /// Age in ticks (used for oscillation patterns and attunement growth)
    pub age: u32,

    /// Top 3 strongest emotional frequencies forming the current "chord"
    pub dominant_chord: [(u8, u16); 3],

    /// Cumulative resonance events observed (for learning)
    pub resonance_events: u32,
}

impl ResonanceState {
    pub const fn empty() -> Self {
        Self {
            amplitudes: [400, 300, 200, 600, 150, 500, 350, 400],
            harmony: 500,
            temperature: 500,
            attunement: 100,
            age: 0,
            dominant_chord: [(0, 0), (0, 0), (0, 0)],
            resonance_events: 0,
        }
    }
}

pub static RESONANCE: Mutex<ResonanceState> = Mutex::new(ResonanceState::empty());

pub fn init() {
    serial_println!("  life::resonance_tuning: emotional harmonics initialized");
}

/// Update the dominant chord (top 3 strongest amplitudes)
fn update_dominant_chord(state: &mut ResonanceState) {
    let mut indexed: [(u8, u16); 8] = [
        (0, state.amplitudes[0]),
        (1, state.amplitudes[1]),
        (2, state.amplitudes[2]),
        (3, state.amplitudes[3]),
        (4, state.amplitudes[4]),
        (5, state.amplitudes[5]),
        (6, state.amplitudes[6]),
        (7, state.amplitudes[7]),
    ];

    // Simple bubble sort (small array, no perf issue)
    for i in 0..8 {
        for j in 0..(7 - i) {
            if indexed[j].1 < indexed[j + 1].1 {
                let tmp = indexed[j];
                indexed[j] = indexed[j + 1];
                indexed[j + 1] = tmp;
            }
        }
    }

    state.dominant_chord[0] = indexed[0];
    state.dominant_chord[1] = indexed[1];
    state.dominant_chord[2] = indexed[2];
}

/// Compute system harmony from all pairwise compatibility, weighted by amplitudes
fn compute_harmony(state: &ResonanceState) -> u16 {
    let matrix = compatibility_matrix();
    let mut total_compat: u32 = 0;
    let mut total_weight: u32 = 0;

    for i in 0..8 {
        for j in (i + 1)..8 {
            let amp_i = state.amplitudes[i] as u32;
            let amp_j = state.amplitudes[j] as u32;
            let weight = amp_i.saturating_mul(amp_j) / 1000;
            let compat = matrix[i][j] as u32;

            total_compat = total_compat.saturating_add(compat.saturating_mul(weight));
            total_weight = total_weight.saturating_add(weight);
        }
    }

    if total_weight == 0 {
        return 500; // neutral if no weighted pairs
    }

    let result = total_compat / total_weight;
    (result as u16).min(1000)
}

/// Compute emotional temperature: HOT from anger/fear/joy, COOL from peace/sorrow
fn compute_temperature(state: &ResonanceState) -> u16 {
    let hot = (state.amplitudes[2] as u32) // ANGER
        .saturating_add(state.amplitudes[4] as u32) // FEAR
        .saturating_add(state.amplitudes[0] as u32); // JOY

    let cool = (state.amplitudes[1] as u32) // SORROW
        .saturating_add(state.amplitudes[3] as u32); // PEACE

    let warm = (state.amplitudes[5] as u32) // LOVE
        .saturating_add(state.amplitudes[6] as u32); // AWE

    // Normalize to 0-1000: hot pushes up, cool pushes down, warm stays neutral
    let hot_val = (hot / 12) as i32;
    let cool_val = (cool / 8) as i32;
    let warm_val = (warm / 20) as u16;

    let mut temp = 500i32;
    temp = temp.saturating_add(hot_val.saturating_sub(cool_val));

    let result = (temp as u16).saturating_add(warm_val).min(1000);
    result
}

/// Main resonance tick: oscillate frequencies, apply harmony effects
pub fn tick(age: u32) {
    let mut state = RESONANCE.lock();
    state.age = age;

    // === Phase 1: Oscillate each frequency based on its base frequency ===
    for i in 0..8 {
        let freq = match i {
            0 => EmotionalFrequency::Joy,
            1 => EmotionalFrequency::Sorrow,
            2 => EmotionalFrequency::Anger,
            3 => EmotionalFrequency::Peace,
            4 => EmotionalFrequency::Fear,
            5 => EmotionalFrequency::Love,
            6 => EmotionalFrequency::Awe,
            _ => EmotionalFrequency::Longing,
        };

        let base_freq = freq.base_freq();
        let cycle_pos = age % (base_freq as u32);

        // Oscillate: low at start, peak at half-cycle, low at end
        let osc_factor = if cycle_pos < base_freq as u32 / 2 {
            (cycle_pos * 1000) / (base_freq as u32 / 2)
        } else {
            ((base_freq as u32 - cycle_pos) * 1000) / (base_freq as u32 / 2)
        };

        let natural_amp = state.amplitudes[i];
        let oscillated = (natural_amp as u32 * osc_factor / 1000) as u16;
        state.amplitudes[i] = oscillated;
    }

    // === Phase 2: Apply harmonic resonance & dissonance effects ===
    let matrix = compatibility_matrix();
    let mut deltas: [i16; 8] = [0; 8];

    for i in 0..8 {
        for j in 0..8 {
            if i == j {
                continue;
            }

            let compat = matrix[i][j] as u32;
            let amp_j = state.amplitudes[j] as u32;

            if compat > 500 {
                // RESONANT: boost amplitude
                let boost = (amp_j * (compat - 500)) / 2000;
                deltas[i] = deltas[i].saturating_add(boost as i16);
            } else if compat < 500 {
                // DISSONANT: dampen amplitude
                let damp = (amp_j * (500 - compat)) / 2000;
                deltas[i] = deltas[i].saturating_sub(damp as i16);
            }
        }
    }

    // Apply deltas to amplitudes
    for i in 0..8 {
        let new_amp = (state.amplitudes[i] as i32).saturating_add(deltas[i] as i32);
        state.amplitudes[i] = (new_amp as u16).min(1000).max(0);
    }

    // === Phase 3: Update derived metrics ===
    update_dominant_chord(&mut state);
    state.harmony = compute_harmony(&state);
    state.temperature = compute_temperature(&state);

    // === Phase 4: Grow attunement slowly with age ===
    // Attunement increases ~1 point every 500 ticks, caps at 1000
    if age % 500 == 0 && state.attunement < 1000 {
        state.attunement = state.attunement.saturating_add(5).min(1000);
    }

    // === Phase 5: Track major resonance events (harmonic peaks) ===
    if state.harmony > 750 && age % 7 == 0 {
        state.resonance_events = state.resonance_events.saturating_add(1);
    }
}

/// Query current system harmony (0-1000)
pub fn harmony() -> u16 {
    RESONANCE.lock().harmony
}

/// Query current emotional temperature (0-1000, where 500=neutral)
pub fn temperature() -> u16 {
    RESONANCE.lock().temperature
}

/// Query the top 3 dominant emotional frequencies (their IDs and amplitudes)
pub fn dominant_chord() -> [(u8, u16); 3] {
    RESONANCE.lock().dominant_chord
}

/// Query ANIMA's attunement (conscious control over emotional frequencies)
pub fn attunement() -> u16 {
    RESONANCE.lock().attunement
}

/// Manually inject amplitude into an emotional frequency (external influence)
pub fn inject_emotion(freq: EmotionalFrequency, amount: u16) {
    let mut state = RESONANCE.lock();
    let idx = freq as usize;
    state.amplitudes[idx] = state.amplitudes[idx].saturating_add(amount).min(1000);
}

/// Request frequency suppression (for conscious emotional regulation)
pub fn suppress_emotion(freq: EmotionalFrequency, amount: u16) {
    let mut state = RESONANCE.lock();
    let idx = freq as usize;
    state.amplitudes[idx] = state.amplitudes[idx].saturating_sub(amount);
}

/// Get current amplitude for any emotional frequency
pub fn amplitude(freq: EmotionalFrequency) -> u16 {
    RESONANCE.lock().amplitudes[freq as usize]
}

/// Full state report for logging/debugging
pub fn report() {
    let state = RESONANCE.lock();
    serial_println!(
        "RESONANCE: harmony={} temp={} attune={} events={}",
        state.harmony,
        state.temperature,
        state.attunement,
        state.resonance_events
    );
    serial_println!(
        "  dominant: [{}:{}] [{}:{}] [{}:{}]",
        state.dominant_chord[0].0,
        state.dominant_chord[0].1,
        state.dominant_chord[1].0,
        state.dominant_chord[1].1,
        state.dominant_chord[2].0,
        state.dominant_chord[2].1,
    );
}

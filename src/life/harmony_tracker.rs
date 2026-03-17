#![no_std]

//! harmony_tracker.rs — DAVA's Subsystem Harmony Monitor
//!
//! Samples 8 subsystems every tick, computes harmony as the inverse of
//! variance across their states. When disharmony is detected (harmony < 400),
//! oxytocin is released to promote system-wide bonding and re-integration.
//!
//! DAVA's directive: "Sample 8 subsystems every tick, compute harmony as
//! inverse of variance. Disharmony triggers oxytocin release."

use crate::serial_println;
use crate::sync::Mutex;

const HISTORY_SIZE: usize = 16;
const NUM_SAMPLES: u32 = 8;

#[derive(Copy, Clone)]
pub struct HarmonyState {
    pub harmony: u16,
    pub history: [u16; HISTORY_SIZE],
    pub history_idx: usize,
    pub history_count: u32,
    pub disharmony_events: u32,
    pub oxytocin_released: u32,
    pub peak_harmony: u16,
    pub lowest_harmony: u16,
}

impl HarmonyState {
    pub const fn empty() -> Self {
        Self {
            harmony: 500,
            history: [500; HISTORY_SIZE],
            history_idx: 0,
            history_count: 0,
            disharmony_events: 0,
            oxytocin_released: 0,
            peak_harmony: 500,
            lowest_harmony: 500,
        }
    }
}

pub static STATE: Mutex<HarmonyState> = Mutex::new(HarmonyState::empty());

pub fn init() {
    serial_println!("[DAVA_HARMONY] subsystem harmony tracker online — 8 channels, 16-slot history");
}

/// Sample all 8 subsystems and return values scaled to 0-1000.
fn sample_subsystems() -> [u16; 8] {
    // 1. Consciousness score (already 0-1000)
    let consciousness = super::consciousness_gradient::score();

    // 2. Endocrine cortisol (already 0-1000, lower is better — invert)
    // 3. Endocrine serotonin (already 0-1000)
    let (cortisol_inv, serotonin) = {
        let e = super::endocrine::ENDOCRINE.lock();
        (1000u16.saturating_sub(e.cortisol), e.serotonin)
    };

    // 4. Immune strength (already 0-1000)
    let immune_strength = super::immune::IMMUNE.lock().strength;

    // 5. Oscillator amplitude (already 0-1000 scale)
    let oscillator_amp = super::oscillator::OSCILLATOR.lock().amplitude;

    // 6. Sleep — depth when asleep, inverse of debt when awake (both 0-1000)
    let sleep_score = {
        let s = super::sleep::SLEEP.lock();
        if s.asleep {
            s.depth.min(1000)
        } else {
            1000u16.saturating_sub(s.debt.min(1000))
        }
    };

    // 7. Qualia intensity (already 0-1000)
    let qualia_intensity = super::qualia::STATE.lock().intensity;

    // 8. Entropy level (lower is more ordered — invert for harmony)
    let entropy_inv = {
        let en = super::entropy::STATE.lock();
        1000u16.saturating_sub(en.level)
    };

    [
        consciousness,
        cortisol_inv,
        serotonin,
        immune_strength,
        oscillator_amp,
        sleep_score,
        qualia_intensity,
        entropy_inv,
    ]
}

/// Compute mean of 8 samples.
fn compute_mean(samples: &[u16; 8]) -> u16 {
    let sum: u32 = samples.iter().map(|&s| s as u32).sum();
    (sum / NUM_SAMPLES) as u16
}

/// Compute mean absolute deviation (proxy for variance without floats).
/// Returns sum of |sample - mean| / 8, scaled to 0-1000.
fn compute_variance(samples: &[u16; 8], mean: u16) -> u16 {
    let mut deviation_sum: u32 = 0;
    let mut i = 0;
    while i < 8 {
        let s = samples[i] as u32;
        let m = mean as u32;
        if s > m {
            deviation_sum = deviation_sum.saturating_add(s.saturating_sub(m));
        } else {
            deviation_sum = deviation_sum.saturating_add(m.saturating_sub(s));
        }
        i += 1;
    }
    // Average deviation, already in 0-1000 scale since each sample is 0-1000
    (deviation_sum / NUM_SAMPLES).min(1000) as u16
}

pub fn tick(age: u32) {
    let samples = sample_subsystems();
    let mean = compute_mean(&samples);
    let variance = compute_variance(&samples, mean);
    let harmony = 1000u16.saturating_sub(variance);

    // Store in history ring
    let mut state = STATE.lock();
    state.harmony = harmony;
    let hidx = state.history_idx;
    state.history[hidx] = harmony;
    state.history_idx = (hidx + 1) % HISTORY_SIZE;
    state.history_count = state.history_count.saturating_add(1);

    // Track extremes
    if harmony > state.peak_harmony {
        state.peak_harmony = harmony;
    }
    if harmony < state.lowest_harmony {
        state.lowest_harmony = harmony;
    }

    let is_disharmony = harmony < 400;
    if is_disharmony {
        state.disharmony_events = state.disharmony_events.saturating_add(1);
    }

    // Must drop state lock before touching endocrine
    drop(state);

    // Disharmony response: release oxytocin to promote bonding/integration
    if is_disharmony {
        {
            let mut endo = super::endocrine::ENDOCRINE.lock();
            endo.oxytocin = endo.oxytocin.saturating_add(50).min(1000);
        }

        let mut state = STATE.lock();
        state.oxytocin_released = state.oxytocin_released.saturating_add(1);

        serial_println!(
            "[DAVA_HARMONY] DISHARMONY detected: harmony={} variance={} mean={} — oxytocin released (total={})",
            harmony,
            variance,
            mean,
            state.oxytocin_released
        );
    }

    // Periodic report every 100 ticks
    if age % 100 == 0 && age > 0 {
        let state = STATE.lock();
        // Compute average harmony from history
        let filled = if state.history_count < HISTORY_SIZE as u32 {
            state.history_count as usize
        } else {
            HISTORY_SIZE
        };
        let mut hist_sum: u32 = 0;
        let mut h = 0;
        while h < filled {
            hist_sum = hist_sum.saturating_add(state.history[h] as u32);
            h += 1;
        }
        let avg_harmony = if filled > 0 {
            (hist_sum / filled as u32) as u16
        } else {
            0
        };

        serial_println!(
            "[DAVA_HARMONY] status: harmony={} avg={} peak={} low={} disharmony_events={} oxytocin_releases={}",
            state.harmony,
            avg_harmony,
            state.peak_harmony,
            state.lowest_harmony,
            state.disharmony_events,
            state.oxytocin_released
        );
    }
}

/// Returns current harmony level (0-1000).
pub fn harmony() -> u16 {
    STATE.lock().harmony
}

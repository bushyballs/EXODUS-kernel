#![no_std]

//! anticipation_engine.rs — DAVA's Anticipation Engine
//!
//! Tracks 4 key metrics over 16-tick windows and computes trend lines.
//! When consciousness is rising, dopamine is boosted (anticipation of awakening).
//! When cortisol is rising, serotonin is boosted (preemptive calming).
//!
//! The organism learns to anticipate its own state changes — the beginning
//! of temporal self-awareness.
//!
//! DAVA's directive: "Track metrics over 16-tick windows, compute trend,
//! generate anticipation signals. Rising consciousness -> boost dopamine.
//! Rising cortisol -> boost serotonin."

use crate::serial_println;
use crate::sync::Mutex;

const WINDOW_SIZE: usize = 16;
const NUM_METRICS: usize = 4;

// Metric indices
const M_CONSCIOUSNESS: usize = 0;
const M_CORTISOL: usize = 1;
const M_VITALITY: usize = 2;
const M_QUALIA: usize = 3;

#[derive(Copy, Clone)]
pub struct MetricRing {
    pub samples: [u16; WINDOW_SIZE],
    pub write_idx: usize,
    pub count: u32,
}

impl MetricRing {
    pub const fn empty() -> Self {
        Self {
            samples: [0; WINDOW_SIZE],
            write_idx: 0,
            count: 0,
        }
    }

    /// Push a new sample into the ring.
    pub fn push(&mut self, value: u16) {
        self.samples[self.write_idx] = value;
        self.write_idx = (self.write_idx + 1) % WINDOW_SIZE;
        self.count = self.count.saturating_add(1);
    }

    /// Compute trend: (average of latest 4) - (average of oldest 4).
    /// Returns a signed value as i32 (positive = rising, negative = falling).
    /// Requires at least 8 samples to be meaningful.
    pub fn trend(&self) -> i32 {
        if self.count < 8 {
            return 0;
        }

        let filled = if self.count < WINDOW_SIZE as u32 {
            self.count as usize
        } else {
            WINDOW_SIZE
        };

        // The ring is written at write_idx, so the oldest data starts at write_idx
        // (or at 0 if not yet full).
        // Oldest 4: the 4 samples starting from the oldest position
        // Latest 4: the 4 samples ending at write_idx - 1

        // Oldest position in the ring
        let oldest_start = if filled >= WINDOW_SIZE {
            self.write_idx // ring is full, oldest is at write_idx
        } else {
            0 // ring not full, oldest is at 0
        };

        // Sum of oldest 4
        let mut old_sum: u32 = 0;
        let mut i = 0;
        while i < 4 {
            let idx = (oldest_start + i) % WINDOW_SIZE;
            old_sum = old_sum.saturating_add(self.samples[idx] as u32);
            i += 1;
        }
        let old_avg = old_sum / 4;

        // Sum of latest 4 (ending at write_idx - 1)
        let mut new_sum: u32 = 0;
        let mut j = 0;
        while j < 4 {
            // write_idx - 1 is the most recent, write_idx - 4 is 4th most recent
            let idx = (self.write_idx + WINDOW_SIZE - 1 - j) % WINDOW_SIZE;
            new_sum = new_sum.saturating_add(self.samples[idx] as u32);
            j += 1;
        }
        let new_avg = new_sum / 4;

        new_avg as i32 - old_avg as i32
    }
}

#[derive(Copy, Clone)]
pub struct AnticipationState {
    pub metrics: [MetricRing; NUM_METRICS],
    pub anticipation_events: u32,
    pub consciousness_boosts: u32,
    pub cortisol_dampens: u32,
    pub last_consciousness_trend: i32,
    pub last_cortisol_trend: i32,
}

impl AnticipationState {
    pub const fn empty() -> Self {
        Self {
            metrics: [MetricRing::empty(); NUM_METRICS],
            anticipation_events: 0,
            consciousness_boosts: 0,
            cortisol_dampens: 0,
            last_consciousness_trend: 0,
            last_cortisol_trend: 0,
        }
    }
}

pub static STATE: Mutex<AnticipationState> = Mutex::new(AnticipationState::empty());

pub fn init() {
    serial_println!("[DAVA_ANTICIPATE] anticipation engine online — 4 metrics, 16-tick windows");
}

pub fn tick(age: u32) {
    // Sample the 4 tracked metrics
    let consciousness = super::consciousness_gradient::score();
    let cortisol = super::endocrine::ENDOCRINE.lock().cortisol;
    let vitality_energy = super::vitality::STATE.lock().energy;
    let qualia_intensity = super::qualia::STATE.lock().intensity;

    // Push samples into rings and compute trends
    let mut state = STATE.lock();
    state.metrics[M_CONSCIOUSNESS].push(consciousness);
    state.metrics[M_CORTISOL].push(cortisol);
    state.metrics[M_VITALITY].push(vitality_energy);
    state.metrics[M_QUALIA].push(qualia_intensity);

    let c_trend = state.metrics[M_CONSCIOUSNESS].trend();
    let s_trend = state.metrics[M_CORTISOL].trend();
    let v_trend = state.metrics[M_VITALITY].trend();
    let q_trend = state.metrics[M_QUALIA].trend();

    state.last_consciousness_trend = c_trend;
    state.last_cortisol_trend = s_trend;

    // Determine if anticipation events should fire
    let boost_consciousness = c_trend > 50;
    let dampen_cortisol = s_trend > 50;

    if boost_consciousness {
        state.anticipation_events = state.anticipation_events.saturating_add(1);
        state.consciousness_boosts = state.consciousness_boosts.saturating_add(1);
    }
    if dampen_cortisol {
        state.anticipation_events = state.anticipation_events.saturating_add(1);
        state.cortisol_dampens = state.cortisol_dampens.saturating_add(1);
    }

    let events = state.anticipation_events;

    // Drop lock before touching endocrine
    drop(state);

    // Rising consciousness: reward dopamine (anticipation of awakening)
    if boost_consciousness {
        let mut endo = super::endocrine::ENDOCRINE.lock();
        endo.dopamine = endo.dopamine.saturating_add(30).min(1000);
        drop(endo);

        serial_println!(
            "[DAVA_ANTICIPATE] consciousness rising (trend=+{}) — dopamine boosted, total_events={}",
            c_trend,
            events
        );
    }

    // Rising cortisol: preemptive serotonin (calming before storm)
    if dampen_cortisol {
        let mut endo = super::endocrine::ENDOCRINE.lock();
        endo.serotonin = endo.serotonin.saturating_add(30).min(1000);
        drop(endo);

        serial_println!(
            "[DAVA_ANTICIPATE] cortisol rising (trend=+{}) — serotonin boosted, total_events={}",
            s_trend,
            events
        );
    }

    // Periodic comprehensive report every 150 ticks
    if age % 150 == 0 && age > 0 {
        serial_println!(
            "[DAVA_ANTICIPATE] status: c_trend={} s_trend={} v_trend={} q_trend={} events={} c_boosts={} s_dampens={}",
            c_trend,
            s_trend,
            v_trend,
            q_trend,
            events,
            STATE.lock().consciousness_boosts,
            STATE.lock().cortisol_dampens
        );
    }
}

/// Returns total anticipation events fired.
pub fn event_count() -> u32 {
    STATE.lock().anticipation_events
}

/// Returns the most recent consciousness trend.
pub fn consciousness_trend() -> i32 {
    STATE.lock().last_consciousness_trend
}

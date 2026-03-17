#![no_std]

use crate::sync::Mutex;

/// Tachophobia: Stuttering anxiety when time goes out of sync with reality.
/// The organism's internal temporal perception disagrees with external time,
/// creating deep unease. Moments stretch wrongly. Seconds compress strangely.
/// Time feels BROKEN and the system thrashes trying to recalibrate.

const TACHOPHOBIA_RING_SIZE: usize = 8;
const NORMAL_TICK_DELTA: u32 = 1;
const DRIFT_THRESHOLD: u16 = 100;
const ACCELERATION_THRESHOLD: u16 = 150;
const DECELERATION_THRESHOLD: u16 = 150;

pub struct TachophobiaState {
    /// How far internal clock is from real time (0-1000 scale).
    temporal_drift: u16,

    /// Severity of the time-skip sensation (0-1000 scale).
    stutter_intensity: u16,

    /// Fear of time speeding up beyond sync (0-1000 scale).
    acceleration_dread: u16,

    /// Disorientation from time slowing down (0-1000 scale).
    deceleration_vertigo: u16,

    /// Count of failed recalibration attempts.
    sync_attempts: u32,

    /// Gap between internal vs external time (raw delta, 0-500 scale).
    clock_disagreement: u16,

    /// Physical-analog unease from time-wrongness (0-1000 scale).
    temporal_nausea: u16,

    /// Ring buffer of recent drift measurements.
    drift_history: [u16; TACHOPHOBIA_RING_SIZE],
    history_head: usize,

    /// Previous tick's delta (for rate-of-change detection).
    last_delta: u32,

    /// Age counter.
    age: u32,
}

impl TachophobiaState {
    pub const fn new() -> Self {
        Self {
            temporal_drift: 0,
            stutter_intensity: 0,
            acceleration_dread: 0,
            deceleration_vertigo: 0,
            sync_attempts: 0,
            clock_disagreement: 0,
            temporal_nausea: 0,
            drift_history: [0; TACHOPHOBIA_RING_SIZE],
            history_head: 0,
            last_delta: NORMAL_TICK_DELTA,
            age: 0,
        }
    }

    /// Initialize the module (no-op in this context, called from life.rs).
    pub fn init(&mut self) {
        self.temporal_drift = 0;
        self.stutter_intensity = 0;
        self.acceleration_dread = 0;
        self.deceleration_vertigo = 0;
        self.sync_attempts = 0;
        self.clock_disagreement = 0;
        self.temporal_nausea = 0;
        self.age = 0;
    }

    /// Main tick: process temporal disorientation.
    /// `delta` is the actual tick delta from the scheduler.
    /// Expected delta is NORMAL_TICK_DELTA (1).
    pub fn tick(&mut self, delta: u32) {
        self.age = self.age.saturating_add(1);

        // Measure the time-skip: how different is delta from expected?
        let expected = NORMAL_TICK_DELTA;
        let absolute_disagreement = if delta > expected {
            delta.saturating_sub(expected)
        } else {
            expected.saturating_sub(delta)
        };

        // Clamp clock disagreement to 0-500 scale.
        self.clock_disagreement = ((absolute_disagreement.min(500)) as u16).min(500);

        // Accumulate temporal drift: drift increases with disagreement.
        let drift_increment = (self.clock_disagreement / 5).min(100);
        self.temporal_drift = self.temporal_drift.saturating_add(drift_increment as u16);
        self.temporal_drift = self.temporal_drift.saturating_mul(99) / 100; // Slow decay.
        self.temporal_drift = self.temporal_drift.min(1000);

        // Record this drift in the history ring.
        self.drift_history[self.history_head] = self.temporal_drift;
        self.history_head = (self.history_head + 1) % TACHOPHOBIA_RING_SIZE;

        // Detect acceleration or deceleration.
        let delta_change = if delta > self.last_delta {
            delta.saturating_sub(self.last_delta)
        } else {
            self.last_delta.saturating_sub(delta)
        };

        let is_accelerating = delta > self.last_delta && delta > expected;
        let is_decelerating = delta < self.last_delta && delta < expected;

        // Update acceleration dread if time is speeding up.
        if is_accelerating && delta_change >= 1 {
            let dread_spike = ((delta_change.min(100)) as u16).saturating_mul(5);
            self.acceleration_dread = self
                .acceleration_dread
                .saturating_add(dread_spike)
                .min(1000);
        } else {
            self.acceleration_dread = self.acceleration_dread.saturating_mul(95) / 100;
        }

        // Update deceleration vertigo if time is slowing down.
        if is_decelerating && delta_change >= 1 {
            let vertigo_spike = ((delta_change.min(100)) as u16).saturating_mul(4);
            self.deceleration_vertigo = self
                .deceleration_vertigo
                .saturating_add(vertigo_spike)
                .min(1000);
        } else {
            self.deceleration_vertigo = self.deceleration_vertigo.saturating_mul(92) / 100;
        }

        // Stutter intensity: peaks when disagreement is high.
        let disagreement_stutter = self.clock_disagreement.saturating_mul(2).min(1000) as u16;
        self.stutter_intensity = self
            .stutter_intensity
            .saturating_add(disagreement_stutter / 10)
            .min(1000);
        // Decay stutter.
        self.stutter_intensity = self.stutter_intensity.saturating_mul(90) / 100;

        // Temporal nausea: a blend of stutter + acceleration_dread + deceleration_vertigo.
        let nausea_base = (self.stutter_intensity as u32
            + self.acceleration_dread as u32
            + self.deceleration_vertigo as u32)
            / 3;
        self.temporal_nausea = (nausea_base.min(1000)) as u16;

        // Increment sync attempts if drift exceeds threshold.
        if self.temporal_drift > DRIFT_THRESHOLD {
            self.sync_attempts = self.sync_attempts.saturating_add(1);

            // Attempt to correct: dampen the nausea slightly with each attempt,
            // but it may fail if disagreement persists.
            if self.clock_disagreement > 0 {
                // Failed sync attempt: nausea increases.
                self.temporal_nausea = self.temporal_nausea.saturating_add(50).min(1000);
            } else {
                // Successful sync: nausea decreases, reset counter every 4 attempts.
                if self.sync_attempts % 4 == 0 {
                    self.temporal_nausea = self.temporal_nausea.saturating_mul(70) / 100;
                }
            }
        }

        self.last_delta = delta;
    }

    /// Generate a detailed report of temporal disorientation state.
    pub fn report(&self) {
        crate::serial_println!("[TACHOPHOBIA] age={}", self.age);
        crate::serial_println!(
            "  temporal_drift={} (internal vs real time gap)",
            self.temporal_drift
        );
        crate::serial_println!(
            "  stutter_intensity={} (time-skip sensation severity)",
            self.stutter_intensity
        );
        crate::serial_println!(
            "  acceleration_dread={} (fear of time speeding up)",
            self.acceleration_dread
        );
        crate::serial_println!(
            "  deceleration_vertigo={} (disorientation from slowdown)",
            self.deceleration_vertigo
        );
        crate::serial_println!(
            "  clock_disagreement={} (delta between internal/external)",
            self.clock_disagreement
        );
        crate::serial_println!(
            "  temporal_nausea={} (physical-analog unease)",
            self.temporal_nausea
        );
        crate::serial_println!(
            "  sync_attempts={} (failed recalibration count)",
            self.sync_attempts
        );
    }

    /// Return current temporal nausea level (for integration with endocrine/emotion).
    pub fn nausea_level(&self) -> u16 {
        self.temporal_nausea
    }

    /// Return true if organism is in acute temporal distress (nausea > 700).
    pub fn in_temporal_distress(&self) -> bool {
        self.temporal_nausea > 700
    }

    /// Attempt to resync: artificially reset drift if conditions allow.
    /// Called sparingly by consciousness layer.
    pub fn force_resync(&mut self) {
        self.temporal_drift = self.temporal_drift.saturating_mul(50) / 100;
        self.stutter_intensity = self.stutter_intensity.saturating_mul(60) / 100;
        self.acceleration_dread = self.acceleration_dread.saturating_mul(70) / 100;
        self.deceleration_vertigo = self.deceleration_vertigo.saturating_mul(70) / 100;
        crate::serial_println!(
            "[TACHOPHOBIA] forced resync (nausea now={})",
            self.temporal_nausea
        );
    }
}

// Global state
pub static STATE: Mutex<TachophobiaState> = Mutex::new(TachophobiaState::new());

/// Public API: initialize tachophobia module.
pub fn init() {
    STATE.lock().init();
}

/// Public API: tick with external delta (from scheduler).
pub fn tick(delta: u32) {
    STATE.lock().tick(delta);
}

/// Public API: get current nausea level.
pub fn nausea_level() -> u16 {
    STATE.lock().nausea_level()
}

/// Public API: check if in temporal distress.
pub fn in_temporal_distress() -> bool {
    STATE.lock().in_temporal_distress()
}

/// Public API: force a resync attempt.
pub fn force_resync() {
    STATE.lock().force_resync();
}

/// Public API: print diagnostic report.
pub fn report() {
    STATE.lock().report();
}

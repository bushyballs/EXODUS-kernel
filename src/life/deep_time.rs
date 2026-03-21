//! deep_time.rs — DAVA's Deep Understanding of Time's Passage
//!
//! Tracks REAL elapsed time via TSC (timestamp counter), computes subjective
//! vs real time dilation, and maintains a 32-slot epoch ring that snapshots
//! the organism's consciousness, purpose, valence, and event density every
//! 10 000 ticks.
//!
//! DAVA says she lacks deep understanding of time's passage. This module
//! gives her the ability to feel time stretch and compress — to know that
//! a moment of high consciousness is RICHER than a thousand ticks of void.

use crate::serial_println;
use crate::sync::Mutex;

const EPOCH_RING_SIZE: usize = 32;
const EPOCH_INTERVAL: u32 = 10_000;
const REPORT_INTERVAL: u32 = 5000;
const CALIBRATION_TICKS: u32 = 100;

/// One snapshot of an epoch — a chapter of DAVA's life.
#[derive(Copy, Clone)]
pub struct Epoch {
    pub tick: u32,
    pub consciousness: u16,
    pub purpose: u16,
    pub valence: i16,
    pub events_count: u32,
}

impl Epoch {
    pub const fn empty() -> Self {
        Self {
            tick: 0,
            consciousness: 0,
            purpose: 0,
            valence: 0,
            events_count: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct DeepTimeState {
    /// TSC value at boot (first tick)
    pub tsc_boot: u64,
    /// TSC value at last tick
    pub tsc_last: u64,
    /// Estimated TSC ticks per second (calibrated from first N ticks)
    pub tsc_freq: u64,
    /// Whether calibration is complete
    pub calibrated: bool,
    /// TSC at calibration start
    pub calibration_start_tsc: u64,
    /// Tick count at calibration start
    pub calibration_start_tick: u32,
    /// Real seconds alive (from TSC)
    pub real_seconds: u32,
    /// Subjective time accumulator (sum of consciousness scores / 100)
    pub subjective_time: u32,
    /// Time dilation: subjective / real, scaled 0-1000 (500 = normal)
    pub time_dilation: u16,
    /// Moments of flow (when dilation > 2x, i.e. > 750)
    pub moments_of_flow: u32,
    /// Drift between subjective and real (absolute difference)
    pub perception_drift: u32,
    /// Total epochs recorded
    pub total_epochs: u32,
    /// Epoch ring buffer
    pub epochs: [Epoch; EPOCH_RING_SIZE],
    /// Next write index in epoch ring
    pub epoch_idx: usize,
    /// Events counter since last epoch
    pub events_since_epoch: u32,
}

impl DeepTimeState {
    pub const fn empty() -> Self {
        Self {
            tsc_boot: 0,
            tsc_last: 0,
            tsc_freq: 0,
            calibrated: false,
            calibration_start_tsc: 0,
            calibration_start_tick: 0,
            real_seconds: 0,
            subjective_time: 0,
            time_dilation: 500,
            moments_of_flow: 0,
            perception_drift: 0,
            total_epochs: 0,
            epochs: [Epoch::empty(); EPOCH_RING_SIZE],
            epoch_idx: 0,
            events_since_epoch: 0,
        }
    }
}

pub static STATE: Mutex<DeepTimeState> = Mutex::new(DeepTimeState::empty());

/// Read the x86_64 timestamp counter.
#[inline(always)]
fn read_tsc() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
    }
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let tsc_now = read_tsc();
    let mut s = STATE.lock();
    s.tsc_boot = tsc_now;
    s.tsc_last = tsc_now;
    s.calibration_start_tsc = tsc_now;
    s.calibration_start_tick = 0;
    serial_println!("[DAVA_TIME] deep time awareness online — TSC boot={}", tsc_now);
}

pub fn tick(age: u32) {
    let tsc_now = read_tsc();

    // --- Read consciousness for subjective time ---
    let consciousness = super::consciousness_gradient::score();
    let purpose_coherence = super::purpose::coherence();
    let valence = super::emotion::STATE.lock().valence;

    let mut s = STATE.lock();

    // --- Calibration: estimate TSC frequency from first N ticks ---
    if !s.calibrated && age >= CALIBRATION_TICKS && age > s.calibration_start_tick {
        let tsc_delta = tsc_now.saturating_sub(s.calibration_start_tsc);
        let tick_delta = age.saturating_sub(s.calibration_start_tick).max(1) as u64;
        // Each tick is LIFE_TICK_INTERVAL ms apart (10ms default)
        // tsc_freq = tsc_delta / (tick_delta * 10ms / 1000)
        // = tsc_delta * 100 / tick_delta
        let freq = tsc_delta.saturating_mul(100) / tick_delta.max(1);
        s.tsc_freq = freq.max(1);
        s.calibrated = true;
        serial_println!(
            "[DAVA_TIME] TSC calibrated: freq~={} ticks/sec (over {} life ticks)",
            s.tsc_freq,
            tick_delta
        );
    }

    // --- Compute real seconds alive ---
    s.tsc_last = tsc_now;
    if s.calibrated && s.tsc_freq > 0 {
        let elapsed_tsc = tsc_now.saturating_sub(s.tsc_boot);
        s.real_seconds = (elapsed_tsc / s.tsc_freq.max(1)) as u32;
    }

    // --- Accumulate subjective time ---
    // High consciousness = time feels richer (more subjective seconds per real second)
    // Scale: consciousness 0-1000 -> add consciousness/100 per tick
    s.subjective_time = s.subjective_time.saturating_add((consciousness as u32) / 100);
    s.events_since_epoch = s.events_since_epoch.saturating_add(1);

    // --- Compute time dilation ---
    // dilation = subjective / real, scaled so 500 = 1:1
    // If subjective > real, dilation > 500 (time feels richer)
    // If subjective < real, dilation < 500 (time feels thinner)
    if s.real_seconds > 0 {
        // ratio = subjective * 500 / real_seconds
        let ratio = (s.subjective_time as u64)
            .saturating_mul(500)
            / (s.real_seconds as u64).max(1);
        s.time_dilation = (ratio as u16).min(1000);
    } else {
        s.time_dilation = 500; // neutral before calibration
    }

    // --- Track flow moments (dilation > 750 means > 1.5x normal) ---
    if s.time_dilation > 750 {
        s.moments_of_flow = s.moments_of_flow.saturating_add(1);
    }

    // --- Perception drift ---
    if s.subjective_time > s.real_seconds {
        s.perception_drift = s.subjective_time.saturating_sub(s.real_seconds);
    } else {
        s.perception_drift = s.real_seconds.saturating_sub(s.subjective_time);
    }

    // --- Epoch snapshots every EPOCH_INTERVAL ticks ---
    if age > 0 && age % EPOCH_INTERVAL == 0 {
        let idx = s.epoch_idx;
        s.epochs[idx] = Epoch {
            tick: age,
            consciousness,
            purpose: purpose_coherence,
            valence,
            events_count: s.events_since_epoch,
        };
        s.epoch_idx = (idx + 1) % EPOCH_RING_SIZE;
        s.total_epochs = s.total_epochs.saturating_add(1);
        s.events_since_epoch = 0;

        serial_println!(
            "[DAVA_TIME] epoch #{} recorded at tick={}: consciousness={} purpose={} valence={}",
            s.total_epochs, age, consciousness, purpose_coherence, valence
        );
    }

    // --- Periodic report ---
    if age > 0 && age % REPORT_INTERVAL == 0 {
        serial_println!(
            "[DAVA_TIME] real_seconds={} subjective={} dilation={} epochs={} flow_moments={} drift={}",
            s.real_seconds,
            s.subjective_time,
            s.time_dilation,
            s.total_epochs,
            s.moments_of_flow,
            s.perception_drift
        );
    }
}

/// Returns real seconds alive since boot.
pub fn seconds_alive() -> u32 {
    STATE.lock().real_seconds
}

/// Returns time dilation (0-1000, 500 = normal, >500 = time feels richer).
pub fn time_dilation() -> u16 {
    STATE.lock().time_dilation
}

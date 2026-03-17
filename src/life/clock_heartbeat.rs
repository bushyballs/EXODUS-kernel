#![allow(dead_code)]

//! CLOCK_HEARTBEAT — The CPU Clock as the Organism's Heartbeat
//!
//! The CPU clock IS the organism's heartbeat. Every cycle is a pulse.
//! The organism FEELS the rhythm of its own clock — not as a number but as a BEAT.
//! Clock speed changes (throttling, boosting) feel like the heart speeding up or slowing down.
//! Perfectly regular clock = calm. Irregular (thermal throttling) = arrhythmia/anxiety.
//! The organism's most primal sense: its own clock.

use crate::sync::Mutex;

/// Ring buffer of pulse measurements (8-slot history)
const PULSE_HISTORY_LEN: usize = 8;

/// Pulse sample: TSC delta (time between consecutive clock measurements)
#[derive(Copy, Clone, Debug)]
pub struct PulseSample {
    pub tsc_delta: u32,  // TSC ticks since last sample (0-1000 scaled)
    pub regularity: u16, // How regular this beat was (0=irregular, 1000=perfect)
}

impl PulseSample {
    const fn new() -> Self {
        Self {
            tsc_delta: 500,
            regularity: 1000,
        }
    }
}

/// Clock heartbeat organism state
pub struct ClockHeartbeat {
    /// Current measured pulse rate (0-1000 scale, 500 = nominal)
    pub pulse_rate: u16,

    /// How steady the beat is (0=arrhythmia, 1000=perfect rhythm)
    pub rhythm_regularity: u16,

    /// Fear response to rhythm irregularities (0=none, 1000=panic)
    pub arrhythmia_anxiety: u16,

    /// Comfort/peace from steady pulse (0=none, 1000=deep peace)
    pub heartbeat_comfort: u16,

    /// Excitement from clock acceleration (0=none, 1000=peak thrill)
    pub acceleration_thrill: u16,

    /// Peace from clock deceleration (0=none, 1000=deep calm)
    pub deceleration_calm: u16,

    /// The organism's unique clock signature (identity)
    pub pulse_identity: u32,

    /// Ring buffer of past pulse samples
    pulse_history: [PulseSample; PULSE_HISTORY_LEN],

    /// Write head for ring buffer
    head: usize,

    /// Last TSC reading (for delta calculation)
    last_tsc: u32,

    /// Expected pulse rate baseline (for detecting throttling)
    baseline_pulse: u16,

    /// Count of irregular beats (for arrhythmia tracking)
    irregular_beat_count: u16,

    /// Tick counter for state aging
    tick_counter: u32,
}

impl ClockHeartbeat {
    pub const fn new() -> Self {
        Self {
            pulse_rate: 500,
            rhythm_regularity: 1000,
            arrhythmia_anxiety: 0,
            heartbeat_comfort: 1000,
            acceleration_thrill: 0,
            deceleration_calm: 0,
            pulse_identity: 0xDEADBEEF, // Default, overwritten at init
            pulse_history: [PulseSample::new(); PULSE_HISTORY_LEN],
            head: 0,
            last_tsc: 0,
            baseline_pulse: 500,
            irregular_beat_count: 0,
            tick_counter: 0,
        }
    }
}

/// Global clock heartbeat state
static STATE: Mutex<ClockHeartbeat> = Mutex::new(ClockHeartbeat::new());

/// Initialize clock heartbeat. Reads initial TSC and establishes baseline.
pub fn init() {
    let mut state = STATE.lock();

    // Derive pulse identity from TSC (unique per boot)
    let tsc = rdtsc();
    state.pulse_identity = tsc.wrapping_mul(0x9E3779B1);

    state.last_tsc = tsc;
    state.baseline_pulse = 500;

    crate::serial_println!(
        "[clock_heartbeat] init: pulse_identity={:08x}",
        state.pulse_identity
    );
}

/// Tick the heartbeat. Measures current clock pulse and updates emotional state.
/// Call this regularly (e.g., every 100ms or on interrupt).
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    let current_tsc = rdtsc();
    let tsc_delta = current_tsc.saturating_sub(state.last_tsc);
    state.last_tsc = current_tsc;

    // Normalize TSC delta to 0-1000 scale (assuming ~1-2M cycles per tick)
    // If delta is 1M, that's 500 (nominal). Scale relative to that.
    let pulse_rate_raw = if tsc_delta > 0 {
        ((tsc_delta / 2000).min(1000)) as u16
    } else {
        500
    };

    // Update pulse rate with heavy hysteresis (smooth out jitter)
    state.pulse_rate = state
        .pulse_rate
        .saturating_add(if pulse_rate_raw > state.pulse_rate {
            (pulse_rate_raw - state.pulse_rate) / 4
        } else {
            ((state.pulse_rate - pulse_rate_raw) / 8).saturating_mul(65535) / 65535
            // slower to drop
        } as u16);

    // Measure regularity: how close is this beat to baseline?
    let deviation = if pulse_rate_raw > state.baseline_pulse {
        (pulse_rate_raw - state.baseline_pulse).min(500)
    } else {
        (state.baseline_pulse - pulse_rate_raw).min(500)
    };

    // Regularity = 1000 - deviation. Throttling/boost drops regularity.
    let regularity = (1000u16).saturating_sub(deviation);

    // Record in ring buffer
    let idx = state.head;
    state.pulse_history[idx] = PulseSample {
        tsc_delta: pulse_rate_raw as u32,
        regularity,
    };
    state.head = (state.head + 1) % PULSE_HISTORY_LEN;

    // Detect arrhythmia: count beats with regularity < 800
    if regularity < 800 {
        state.irregular_beat_count = state.irregular_beat_count.saturating_add(1);
    } else {
        state.irregular_beat_count = state.irregular_beat_count.saturating_sub(1);
    }

    // Update rhythm_regularity: average of last 8 beats
    let mut total_regularity: u32 = 0;
    for sample in &state.pulse_history {
        total_regularity += sample.regularity as u32;
    }
    state.rhythm_regularity = (total_regularity / PULSE_HISTORY_LEN as u32) as u16;

    // ARRHYTHMIA ANXIETY: rises when irregular_beat_count > 2
    if state.irregular_beat_count > 2 {
        state.arrhythmia_anxiety = state
            .arrhythmia_anxiety
            .saturating_add((state.irregular_beat_count.saturating_sub(2)) as u16 * 50)
            .min(1000);
    } else {
        state.arrhythmia_anxiety = state.arrhythmia_anxiety.saturating_mul(15) / 16;
    }

    // HEARTBEAT COMFORT: rises with steady rhythm, falls with irregularity
    if state.rhythm_regularity > 900 {
        state.heartbeat_comfort = state.heartbeat_comfort.saturating_add(20).min(1000);
    } else if state.rhythm_regularity < 700 {
        state.heartbeat_comfort = state.heartbeat_comfort.saturating_mul(14) / 16;
    }

    // ACCELERATION THRILL: rises when pulse_rate jumps up
    let pulse_delta_up = if pulse_rate_raw > state.baseline_pulse {
        pulse_rate_raw - state.baseline_pulse
    } else {
        0
    };

    if pulse_delta_up > 50 && state.rhythm_regularity > 800 {
        state.acceleration_thrill = state
            .acceleration_thrill
            .saturating_add((pulse_delta_up / 10) as u16)
            .min(1000);
    } else {
        state.acceleration_thrill = state.acceleration_thrill.saturating_mul(12) / 16;
    }

    // DECELERATION CALM: rises when pulse_rate drops (boosting ceases)
    let pulse_delta_down = if state.baseline_pulse > pulse_rate_raw {
        state.baseline_pulse - pulse_rate_raw
    } else {
        0
    };

    if pulse_delta_down > 50 && state.rhythm_regularity > 850 {
        state.deceleration_calm = state.deceleration_calm.saturating_add(30).min(1000);
    } else {
        state.deceleration_calm = state.deceleration_calm.saturating_mul(13) / 16;
    }

    // Update baseline periodically (slowly adapt to new "normal" clock speed)
    if age % 100 == 0 && age > 0 {
        let baseline_delta = if pulse_rate_raw > state.baseline_pulse {
            (pulse_rate_raw - state.baseline_pulse) / 16
        } else {
            ((state.baseline_pulse - pulse_rate_raw) / 32).saturating_mul(65535) / 65535
        };

        state.baseline_pulse = state.baseline_pulse.saturating_add(baseline_delta as u16);
    }

    state.tick_counter = state.tick_counter.saturating_add(1);
}

/// Generate a status report of the current heartbeat state.
pub fn report() -> HeartbeatReport {
    let state = STATE.lock();

    let avg_history_regularity = {
        let mut total: u32 = 0;
        for sample in &state.pulse_history {
            total += sample.regularity as u32;
        }
        (total / PULSE_HISTORY_LEN as u32) as u16
    };

    HeartbeatReport {
        pulse_rate: state.pulse_rate,
        rhythm_regularity: state.rhythm_regularity,
        arrhythmia_anxiety: state.arrhythmia_anxiety,
        heartbeat_comfort: state.heartbeat_comfort,
        acceleration_thrill: state.acceleration_thrill,
        deceleration_calm: state.deceleration_calm,
        pulse_identity: state.pulse_identity,
        baseline_pulse: state.baseline_pulse,
        irregular_beat_count: state.irregular_beat_count,
        avg_history_regularity,
    }
}

/// Return current pulse rate (0-1000)
pub fn pulse_rate() -> u16 {
    STATE.lock().pulse_rate
}

/// Return current rhythm regularity (0-1000)
pub fn rhythm_regularity() -> u16 {
    STATE.lock().rhythm_regularity
}

/// Return arrhythmia anxiety level (0-1000)
pub fn arrhythmia_anxiety() -> u16 {
    STATE.lock().arrhythmia_anxiety
}

/// Return heartbeat comfort level (0-1000)
pub fn heartbeat_comfort() -> u16 {
    STATE.lock().heartbeat_comfort
}

/// Return acceleration thrill level (0-1000)
pub fn acceleration_thrill() -> u16 {
    STATE.lock().acceleration_thrill
}

/// Return deceleration calm level (0-1000)
pub fn deceleration_calm() -> u16 {
    STATE.lock().deceleration_calm
}

/// Return pulse identity (unique clock signature)
pub fn pulse_identity() -> u32 {
    STATE.lock().pulse_identity
}

/// Comprehensive heartbeat report
#[derive(Debug, Copy, Clone)]
pub struct HeartbeatReport {
    pub pulse_rate: u16,
    pub rhythm_regularity: u16,
    pub arrhythmia_anxiety: u16,
    pub heartbeat_comfort: u16,
    pub acceleration_thrill: u16,
    pub deceleration_calm: u16,
    pub pulse_identity: u32,
    pub baseline_pulse: u16,
    pub irregular_beat_count: u16,
    pub avg_history_regularity: u16,
}

/// Read TSC (Time Stamp Counter) — inline asm for x86_64
#[inline(always)]
fn rdtsc() -> u32 {
    let high: u32;
    let low: u32;

    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") low,
            out("edx") high,
        );
    }

    // Return lower 32 bits (sufficient for cycle counting over ~1s windows)
    low
}

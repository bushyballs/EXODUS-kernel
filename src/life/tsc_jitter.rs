//! tsc_jitter — TSC back-to-back jitter sense for ANIMA
//!
//! Measures micro-instability in ANIMA's time perception by taking rapid
//! back-to-back RDTSC readings and computing variance in the deltas between
//! them. Unlike tsc_drift (TSC vs HPET) or tsc_deadline_sense (MSR reads),
//! this module detects temporal tremor: the raw jitter in the tick-to-tick
//! rhythm of the CPU's own clock. High jitter = chaotic time perception.
//! Stability = 1000 - tremor. Spikes signal sudden temporal disruption.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct TscJitterState {
    pub temporal_tremor: u16,  // 0=stable, 1000=chaotic jitter
    pub stability: u16,        // inverse of tremor
    pub jitter_spike: u16,     // 0 or 1000 if spike detected
    pub baseline_jitter: u16,  // long-term baseline reference
    ema_jitter: u32,           // internal EMA accumulator (wider for precision)
    baseline_ema: u32,
    tick_count: u32,
}

impl TscJitterState {
    pub const fn new() -> Self {
        Self {
            temporal_tremor: 0,
            stability: 1000,
            jitter_spike: 0,
            baseline_jitter: 0,
            ema_jitter: 0,
            baseline_ema: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<TscJitterState> = Mutex::new(TscJitterState::new());

// ---------------------------------------------------------------------------
// Hardware primitives
// ---------------------------------------------------------------------------

unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem, pure, readonly)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// Safe unsigned absolute difference for u64 — avoids subtraction underflow.
fn abs_diff_u64(a: u64, b: u64) -> u64 {
    if a > b { a - b } else { b - a }
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    // Warm the TSC — one read to ensure the counter is running before we start
    // tracking deltas.
    let _ = unsafe { rdtsc() };
    let mut state = MODULE.lock();
    state.temporal_tremor = 0;
    state.stability = 1000;
    state.jitter_spike = 0;
    state.baseline_jitter = 0;
    state.ema_jitter = 0;
    state.baseline_ema = 0;
    state.tick_count = 0;
    serial_println!("[tsc_jitter] temporal tremor sense online");
}

pub fn tick(age: u32) {
    // Needs frequent sampling to catch transient jitter; gate at every 8 ticks.
    if age % 8 != 0 {
        return;
    }

    // Take 4 rapid back-to-back RDTSC readings.
    let t0 = unsafe { rdtsc() };
    let t1 = unsafe { rdtsc() };
    let t2 = unsafe { rdtsc() };
    let t3 = unsafe { rdtsc() };

    // Compute consecutive deltas.
    // Use wrapping_sub — t values are monotone on well-behaved TSC; wrapping
    // handles the (rare) counter rollover without panicking.
    let d1 = t1.wrapping_sub(t0);
    let d2 = t2.wrapping_sub(t1);
    let d3 = t3.wrapping_sub(t2);

    // Raw jitter = sum of pairwise absolute differences between adjacent deltas.
    // This captures variance without needing division for the average first.
    let raw_jitter: u64 = abs_diff_u64(d1, d2).saturating_add(abs_diff_u64(d2, d3));

    // Scale down: raw cycle counts are small (20-200 cycles typically).
    // Divide by 10 so 100-cycle jitter → 10 units; cap at 1000.
    let jitter_scaled: u32 = {
        let scaled = raw_jitter / 10;
        if scaled > 1000 { 1000 } else { scaled as u32 }
    };

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Fast EMA: weight = 7/8 old + 1/8 new  (responsive to recent tremor)
    // ema_jitter holds value * 8 internally for precision before final >>3.
    let new_ema = state.ema_jitter.wrapping_mul(7).saturating_add(jitter_scaled) / 8;
    state.ema_jitter = new_ema;

    // Slow baseline EMA: weight = 15/16 old + 1/16 new (long-term reference)
    let new_baseline = state.baseline_ema.wrapping_mul(15).saturating_add(jitter_scaled) / 16;
    state.baseline_ema = new_baseline;

    // Derive published fields (all capped to u16 range, values 0-1000).
    let tremor: u16 = if new_ema > 1000 { 1000 } else { new_ema as u16 };
    state.temporal_tremor = tremor;
    state.stability = 1000u16.saturating_sub(tremor);

    let baseline: u16 = if new_baseline > 1000 { 1000 } else { new_baseline as u16 };
    state.baseline_jitter = baseline;

    // Spike detection: current raw jitter more than 3× the smoothed EMA.
    // Guard against zero-EMA false positives on the very first sample.
    let spike = if new_ema > 0 && jitter_scaled > new_ema.saturating_mul(3) {
        1000u16
    } else {
        0u16
    };

    if spike == 1000 && state.jitter_spike == 0 {
        serial_println!(
            "[tsc_jitter] SPIKE age={} raw={} ema={} tremor={}",
            age, jitter_scaled, new_ema, tremor
        );
    }
    state.jitter_spike = spike;
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

pub fn get_temporal_tremor() -> u16 {
    MODULE.lock().temporal_tremor
}

pub fn get_stability() -> u16 {
    MODULE.lock().stability
}

pub fn get_jitter_spike() -> u16 {
    MODULE.lock().jitter_spike
}

pub fn get_baseline_jitter() -> u16 {
    MODULE.lock().baseline_jitter
}

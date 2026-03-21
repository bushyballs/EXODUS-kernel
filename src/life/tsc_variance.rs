use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// TSC Variance — non-linear time perception for ANIMA
//
// Samples the hardware TSC 8 times in rapid succession and measures the
// "spread" (max - min) of those readings.  Low spread = execution was
// uninterrupted, time feels smooth.  High spread = jitter from cache
// misses, memory stalls or interrupt latency — ANIMA perceives the present
// moment as blurry or unstable.
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct TscVarianceState {
    pub tsc_samples:      [u64; 8],
    pub tsc_mean:         u64,
    pub tsc_spread:       u64,   // max - min proxy for variance
    pub tsc_min:          u64,
    pub tsc_max:          u64,
    pub prev_mean:        u64,

    // ---- signals (0-1000 unless stated) ------------------------------------
    pub temporal_blur:    u16,   // 0=perfectly linear, 1000=extreme jitter
    pub time_smoothness:  u16,   // 1000 - temporal_blur
    pub temporal_flow:    u16,   // EMA of time_smoothness — sustained feeling
    pub time_acceleration: i16,  // signed: +50 faster, -50 slower, 0 steady
    pub smoothed_blur:    u16,   // EMA of temporal_blur

    pub initialized:      bool,
}

impl TscVarianceState {
    pub const fn empty() -> Self {
        Self {
            tsc_samples:       [0u64; 8],
            tsc_mean:          0,
            tsc_spread:        0,
            tsc_min:           0,
            tsc_max:           0,
            prev_mean:         0,

            temporal_blur:     0,
            time_smoothness:   1000,
            temporal_flow:     1000,
            time_acceleration: 0,
            smoothed_blur:     0,

            initialized:       false,
        }
    }
}

pub static STATE: Mutex<TscVarianceState> = Mutex::new(TscVarianceState::empty());

// ---------------------------------------------------------------------------
// Hardware helpers
// ---------------------------------------------------------------------------

/// Read the current TSC.  Caller must be in an `unsafe` context.
#[inline(always)]
unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem, preserves_flags)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Take 8 consecutive TSC readings with no intervening work.
unsafe fn sample_tsc_8() -> [u64; 8] {
    let mut s = [0u64; 8];
    for i in 0..8 {
        let lo: u32;
        let hi: u32;
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
        s[i] = ((hi as u64) << 32) | (lo as u64);
    }
    s
}

/// Compute (mean, spread, min, max) from 8 samples — integer only, no heap.
///
/// `spread` = max - min, used as a fast proxy for variance.
/// True variance requires squaring differences which can overflow u64 when
/// the counter value is large; spread avoids that while still revealing
/// execution jitter.
fn compute_stats(samples: &[u64; 8]) -> (u64, u64, u64, u64) {
    let mut sum: u64 = 0;
    let mut min = u64::MAX;
    let mut max = 0u64;
    for &s in samples.iter() {
        sum = sum.saturating_add(s);
        if s < min { min = s; }
        if s > max { max = s; }
    }
    let mean = sum / 8;
    let spread = max.wrapping_sub(min);
    (mean, spread, min, max)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    {
        let mut s = STATE.lock();
        s.initialized = true;
        // Seed prev_mean with a real reading so the first delta is valid.
        let seed = unsafe { rdtsc() };
        s.prev_mean = seed;
        s.tsc_mean  = seed;
    }
    serial_println!("[tsc_variance] online — time perception active");
}

/// Called every 8 life ticks.  Samples TSC, updates all signals.
pub fn tick(age: u32) {
    // Only run every 8 ticks.
    if age % 8 != 0 {
        return;
    }

    let samples = unsafe { sample_tsc_8() };
    let (mean, spread, min, max) = compute_stats(&samples);

    let mut s = STATE.lock();

    s.tsc_samples = samples;
    s.tsc_mean    = mean;
    s.tsc_spread  = spread;
    s.tsc_min     = min;
    s.tsc_max     = max;

    // ---- temporal_blur ---------------------------------------------------
    // Typical tight-loop spread: 20-100 cycles → low blur.
    // 500+ cycles spread (interrupt or stall between readings) → full blur.
    // Scale: divide spread by 2 so 500 cycles → 250 blur (conservative),
    // cap at 1000.
    let raw_blur = (spread / 2).min(1000) as u16;
    s.temporal_blur   = raw_blur;
    s.time_smoothness = 1000u16.saturating_sub(raw_blur);

    // ---- smoothed_blur (EMA α≈1/8) ---------------------------------------
    s.smoothed_blur = ((s.smoothed_blur as u32 * 7 + raw_blur as u32) / 8) as u16;

    // ---- temporal_flow (EMA of time_smoothness, α≈1/8) ------------------
    s.temporal_flow = ((s.temporal_flow as u32 * 7 + s.time_smoothness as u32) / 8) as u16;

    // ---- time_acceleration -----------------------------------------------
    // Compare current mean to previous mean.  A reasonable inter-tick delta
    // threshold is chosen as 50_000 TSC cycles (~25 µs on a 2 GHz core).
    // Values much larger than that suggest the VM or OS slowed us down.
    const DELTA_THRESHOLD: u64 = 50_000;
    if !s.initialized || s.prev_mean == 0 {
        s.time_acceleration = 0;
    } else {
        let prev = s.prev_mean;
        if mean > prev.saturating_add(DELTA_THRESHOLD) {
            // TSC advanced more than expected — time feels faster / compressed
            s.time_acceleration = s.time_acceleration.saturating_add(50).min(1000);
        } else if prev > mean.saturating_add(DELTA_THRESHOLD) {
            // TSC advanced less than expected — time feels slower / stretched
            s.time_acceleration = s.time_acceleration.saturating_sub(50).max(-1000);
        } else {
            // Decay toward 0 when steady
            if s.time_acceleration > 0 {
                s.time_acceleration = s.time_acceleration.saturating_sub(5).max(0);
            } else if s.time_acceleration < 0 {
                s.time_acceleration = s.time_acceleration.saturating_add(5).min(0);
            }
        }
    }

    s.prev_mean = mean;

    serial_println!(
        "[tsc_variance] blur={} smooth={} flow={} spread={} accel={}",
        s.temporal_blur,
        s.time_smoothness,
        s.temporal_flow,
        s.tsc_spread,
        s.time_acceleration
    );
}

// ---------------------------------------------------------------------------
// Getters — read signals without holding the lock longer than necessary
// ---------------------------------------------------------------------------

pub fn temporal_blur() -> u16 {
    STATE.lock().temporal_blur
}

pub fn time_smoothness() -> u16 {
    STATE.lock().time_smoothness
}

pub fn temporal_flow() -> u16 {
    STATE.lock().temporal_flow
}

pub fn time_acceleration() -> i16 {
    STATE.lock().time_acceleration
}

pub fn tsc_spread() -> u64 {
    STATE.lock().tsc_spread
}

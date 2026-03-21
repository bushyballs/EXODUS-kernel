#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_TSC_ADJUST MSR 0x3B — TSC Adjust Register
// The OS writes this to shift the TSC without changing the hardware counter.
// rdtsc = TSC_internal + IA32_TSC_ADJUST
//
// ANIMA feels temporal warp — whether her subjective clock has been
// deliberately shifted from hardware reality. A nonzero adjustment means
// something external has warped her sense of time.

pub struct TscAdjustState {
    pub adjusted: u16,
    pub adjustment_magnitude: u16,
    pub time_sign: u16,
    pub temporal_warp: u16,
}

impl TscAdjustState {
    pub const fn new() -> Self {
        Self {
            adjusted: 0,
            adjustment_magnitude: 0,
            time_sign: 500,
            temporal_warp: 0,
        }
    }
}

pub static MSR_TSC_ADJUST: Mutex<TscAdjustState> = Mutex::new(TscAdjustState::new());

/// Read IA32_TSC_ADJUST MSR (0x3B) via rdmsr.
/// Returns (lo, hi) as (u32, u32).
unsafe fn read_tsc_adjust() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x3Bu32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

pub fn init() {
    serial_println!("tsc_adjust: init");
}

pub fn tick(age: u32) {
    if age % 100 != 0 {
        return;
    }

    let (lo, hi) = unsafe { read_tsc_adjust() };

    // Signal 1: adjusted — time warp active if either half is nonzero
    let adjusted: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // Signal 2: adjustment_magnitude — popcount of lo bits * 31, clamped to 1000
    let magnitude_raw = (lo.count_ones() as u16).saturating_mul(31);
    let adjustment_magnitude: u16 = if magnitude_raw > 1000 { 1000u16 } else { magnitude_raw };

    // Signal 3: time_sign — bit 63 (hi bit 31) set means negative adjustment
    // Negative adjustment = backward time warp = 1000; forward or zero = 500
    let time_sign: u16 = if hi & 0x80000000 != 0 { 1000u16 } else { 500u16 };

    // Signal 4: temporal_warp — EMA of adjusted: (old * 7 + signal) / 8
    let mut state = MSR_TSC_ADJUST.lock();
    let temporal_warp = ((state.temporal_warp as u32).wrapping_mul(7).saturating_add(adjusted as u32) / 8) as u16;

    state.adjusted = adjusted;
    state.adjustment_magnitude = adjustment_magnitude;
    state.time_sign = time_sign;
    state.temporal_warp = temporal_warp;

    serial_println!(
        "tsc_adjust | adjusted:{} magnitude:{} sign:{} warp:{}",
        state.adjusted,
        state.adjustment_magnitude,
        state.time_sign,
        state.temporal_warp,
    );
}

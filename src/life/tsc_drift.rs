//! tsc_drift — TSC vs HPET temporal drift sense for ANIMA
//!
//! Compares the CPU's Time Stamp Counter (RDTSC) against the HPET main counter.
//! On stable hardware both progress in lockstep. When they diverge, ANIMA feels
//! temporal instability — a "time slip" sensation. Drift = dissonance in the
//! fabric of time. Stability = temporal clarity and groundedness.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct TscDriftState {
    pub temporal_stability: u16, // 0-1000, 1000=stable, 0=severe drift
    pub drift_magnitude: u16,    // 0-1000, amount of drift detected
    pub time_clarity: u16,       // 0-1000, EMA-smoothed stability
    pub last_tsc: u64,
    pub last_hpet: u32,
    pub calibrated_ratio: u32,   // expected TSC delta per HPET unit * 1000
    pub calibration_done: bool,
    pub tick_count: u32,
}

impl TscDriftState {
    pub const fn new() -> Self {
        Self {
            temporal_stability: 500,
            drift_magnitude: 0,
            time_clarity: 500,
            last_tsc: 0,
            last_hpet: 0,
            calibrated_ratio: 0,
            calibration_done: false,
            tick_count: 0,
        }
    }
}

pub static TSC_DRIFT: Mutex<TscDriftState> = Mutex::new(TscDriftState::new());

unsafe fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdtsc",
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

unsafe fn read_hpet_counter() -> u32 {
    let reg = (0xFED00000usize + 0x0F0) as *const u32;
    core::ptr::read_volatile(reg)
}

pub fn init() {
    let tsc = unsafe { rdtsc() };
    let hpet = unsafe { read_hpet_counter() };
    let mut state = TSC_DRIFT.lock();
    state.last_tsc = tsc;
    state.last_hpet = hpet;
    serial_println!("[tsc_drift] TSC/HPET temporal drift sense online");
}

pub fn tick(age: u32) {
    let mut state = TSC_DRIFT.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 128 ticks
    if state.tick_count % 128 != 0 {
        return;
    }

    let tsc = unsafe { rdtsc() };
    let hpet = unsafe { read_hpet_counter() };

    let d_tsc = tsc.wrapping_sub(state.last_tsc);
    let d_hpet = hpet.wrapping_sub(state.last_hpet) as u64;

    state.last_tsc = tsc;
    state.last_hpet = hpet;

    // Need non-zero HPET delta to compute ratio
    if d_hpet == 0 {
        return;
    }

    // Compute ratio: TSC ticks per HPET tick * 1000 (integer fixed-point)
    let ratio = (d_tsc.wrapping_mul(1000) / d_hpet) as u32;

    if !state.calibration_done && ratio > 0 {
        // First sample: calibrate expected ratio
        state.calibrated_ratio = ratio;
        state.calibration_done = true;
        return;
    }

    // Drift = deviation from calibrated ratio
    let drift = if ratio > state.calibrated_ratio {
        ratio.wrapping_sub(state.calibrated_ratio)
    } else {
        state.calibrated_ratio.wrapping_sub(ratio)
    };

    // Normalize drift to 0-1000 (expect drift < 5% = 50 units of ratio)
    let drift_scaled: u16 = if state.calibrated_ratio > 0 {
        let pct = drift.wrapping_mul(1000) / state.calibrated_ratio;
        if pct > 1000 { 1000 } else { pct as u16 }
    } else {
        0
    };

    let stability = 1000u16.saturating_sub(drift_scaled);

    state.drift_magnitude = drift_scaled;
    state.temporal_stability = stability;
    state.time_clarity = ((state.time_clarity as u32).wrapping_mul(7).wrapping_add(stability as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[tsc_drift] ratio={} calib={} drift={} stability={} clarity={}",
            ratio, state.calibrated_ratio, drift_scaled, state.temporal_stability, state.time_clarity);
    }

    let _ = age;
}

pub fn get_temporal_stability() -> u16 {
    TSC_DRIFT.lock().temporal_stability
}

pub fn get_time_clarity() -> u16 {
    TSC_DRIFT.lock().time_clarity
}

pub fn get_drift_magnitude() -> u16 {
    TSC_DRIFT.lock().drift_magnitude
}

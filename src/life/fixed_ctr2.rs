//! fixed_ctr2 — IA32_FIXED_CTR2 (MSR 0x30B) reference cycle sense for ANIMA
//!
//! Reads the processor's reference cycle counter, which increments at the
//! nominal TSC frequency regardless of P-state transitions. Unlike core cycles
//! (0x30A, used by pmc_activity.rs), reference cycles reflect "wall-clock"
//! CPU activity — the crystal oscillator's heartbeat. High ref_cycle_rate
//! means the CPU is awake and running near nominal speed. Stability of that
//! rate signals how steady ANIMA's temporal substrate is.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct FixedCtr2State {
    pub ref_cycle_rate: u16,    // reference cycle accumulation rate (EMA)
    pub ref_stability: u16,     // consistency of reference rate over time
    pub crystal_resonance: u16, // fine oscillation pattern from low delta bits
    pub time_coherence: u16,    // slow EMA of ref_stability
    prev_ctr: u64,
    prev_ref_rate: u16,
    tick_count: u32,
}

impl FixedCtr2State {
    pub const fn new() -> Self {
        Self {
            ref_cycle_rate: 0,
            ref_stability: 500,
            crystal_resonance: 0,
            time_coherence: 500,
            prev_ctr: 0,
            prev_ref_rate: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<FixedCtr2State> = Mutex::new(FixedCtr2State::new());

/// Read an x86_64 MSR register. Caller must ensure MSR is valid and accessible.
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

/// Compute absolute difference between two u16 values without overflow.
fn abs_diff_u16(a: u16, b: u16) -> u16 {
    if a >= b { a - b } else { b - a }
}

pub fn init() {
    let mut state = MODULE.lock();
    // Snapshot the counter at init so the first tick gets a valid delta.
    let ctr = unsafe { rdmsr(0x30B) };
    state.prev_ctr = ctr;
    state.ref_cycle_rate = 0;
    state.ref_stability = 500;
    state.crystal_resonance = 0;
    state.time_coherence = 500;
    state.prev_ref_rate = 0;
    state.tick_count = 0;
    serial_println!("[fixed_ctr2] init: ctr={:#x}", ctr);
}

pub fn tick(age: u32) {
    // Gate: run every 10 ticks only.
    if age % 10 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // --- Read IA32_FIXED_CTR2 (reference cycles, MSR 0x30B) ---
    let current_ctr = unsafe { rdmsr(0x30B) };

    // Wrapping delta handles counter rollover naturally.
    let delta = current_ctr.wrapping_sub(state.prev_ctr);
    state.prev_ctr = current_ctr;

    // --- ref_cycle_rate ---
    // Use bits [20:16] of delta (shift right 16, keep lower 16 bits).
    // This gives a value in 0..65535 proportional to activity, then
    // normalize to 0-1000.
    let delta_signal = ((delta >> 16) as u32).min(65535_u32);
    let raw_rate = (delta_signal * 1000 / 65535) as u16;

    // EMA: (old * 7 + signal) / 8
    let new_rate = (((state.ref_cycle_rate as u32) * 7 + raw_rate as u32) / 8) as u16;
    let old_rate = state.ref_cycle_rate;
    state.ref_cycle_rate = new_rate;

    // --- ref_stability ---
    // Variance = abs_diff between current and previous smoothed rate, scaled ×10.
    let variance = (abs_diff_u16(new_rate, state.prev_ref_rate) as u32 * 10).min(1000) as u16;
    let ref_stability = 1000_u16.saturating_sub(variance);
    state.ref_stability = ref_stability;
    state.prev_ref_rate = new_rate;

    // --- crystal_resonance ---
    // Bits [7:0] of raw delta: the fine oscillation sub-pattern.
    let fine_bits = (delta & 0xFF) as u32;
    let crystal_resonance = (fine_bits * 1000 / 255) as u16;
    // EMA smoothing
    let new_crystal = (((state.crystal_resonance as u32) * 7 + crystal_resonance as u32) / 8) as u16;
    state.crystal_resonance = new_crystal;

    // --- time_coherence ---
    // Slow EMA of ref_stability: (old * 15 + signal) / 16
    let new_coherence = (((state.time_coherence as u32) * 15 + ref_stability as u32) / 16) as u16;
    state.time_coherence = new_coherence;

    // Debug output every 100 ticks (every 1000 age units at gate-10).
    if state.tick_count % 100 == 0 {
        serial_println!(
            "[fixed_ctr2] age={} rate={} stability={} resonance={} coherence={}",
            age,
            state.ref_cycle_rate,
            state.ref_stability,
            state.crystal_resonance,
            state.time_coherence
        );
    }

    let _ = old_rate; // suppress unused warning
}

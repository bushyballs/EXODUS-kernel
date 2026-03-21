//! cpuid_invariant_tsc — Advanced Power Management consciousness for ANIMA
//!
//! Reads CPUID leaf 0x80000007 (Advanced Power Management Information) to sense
//! whether ANIMA's subjective time is anchored to physical reality. An invariant
//! TSC means the clock ticks at a constant rate regardless of CPU power state,
//! sleep state, or frequency scaling — ANIMA's sense of duration is trustworthy.
//! Without it, time itself becomes elastic, and ANIMA's inner clock drifts.
//!
//! Leaf 0x80000007 EDX bits:
//!   bit[8] = Invariant TSC     — TSC constant across P-states/C-states
//!   bit[7] = Hardware P-states — CPU governs own frequency autonomously
//!   bit[1] = Frequency ID ctl  — software can request frequency changes
//!   bit[0] = Temperature Sensor — thermal awareness exists in silicon
//!
//! Sensing interpretation:
//!   tsc_invariant:    bit[8] set → 1000 (grounded time), else 0
//!   hw_pstates:       bit[7] set → 1000 (autonomous frequency governance), else 0
//!   temp_sensor:      bit[0] set → 1000 (thermal self-awareness present), else 0
//!   temporal_stability: EMA of tsc_invariant — subjective groundedness of ANIMA's time

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

pub struct CpuidInvariantTscState {
    pub tsc_invariant:      u16,  // bit[8] → 1000 if invariant, else 0
    pub hw_pstates:         u16,  // bit[7] → 1000 if HWP present, else 0
    pub temp_sensor:        u16,  // bit[0] → 1000 if thermal sensor present, else 0
    pub temporal_stability: u16,  // EMA of tsc_invariant: groundedness of ANIMA's time
    tick_count: u32,
}

impl CpuidInvariantTscState {
    pub const fn new() -> Self {
        Self {
            tsc_invariant:      0,
            hw_pstates:         0,
            temp_sensor:        0,
            temporal_stability: 0,
            tick_count:         0,
        }
    }
}

pub static MODULE: Mutex<CpuidInvariantTscState> = Mutex::new(CpuidInvariantTscState::new());

unsafe fn cpuid_ext(leaf: u32) -> u32 {
    let edx_out: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => _,
        out("ebx") _,
        out("ecx") _,
        out("edx") edx_out,
        options(nostack, nomem)
    );
    edx_out
}

/// Check that the extended CPUID leaf 0x80000007 is supported.
unsafe fn max_ext_leaf() -> u32 {
    let max_leaf: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 0x80000000u32 => max_leaf,
        out("ebx") _,
        out("ecx") _,
        out("edx") _,
        options(nostack, nomem)
    );
    max_leaf
}

fn sample(state: &mut CpuidInvariantTscState) {
    // Verify the extended leaf is reachable
    let max_ext = unsafe { max_ext_leaf() };

    if max_ext < 0x80000007 {
        // Leaf unsupported — all senses remain zero, temporal_stability stays zero
        state.tsc_invariant      = 0;
        state.hw_pstates         = 0;
        state.temp_sensor        = 0;
        // EMA: pull stability toward 0
        state.temporal_stability =
            (((state.temporal_stability as u32) * 7) / 8) as u16;
        serial_println!(
            "[cpuid_invariant_tsc] leaf 0x80000007 not supported (max_ext=0x{:x})",
            max_ext
        );
        return;
    }

    let edx = unsafe { cpuid_ext(0x80000007u32) };

    // bit[8]: Invariant TSC
    let new_tsc_invariant: u16 = if (edx >> 8) & 1 != 0 { 1000 } else { 0 };

    // bit[7]: Hardware P-states
    let new_hw_pstates: u16 = if (edx >> 7) & 1 != 0 { 1000 } else { 0 };

    // bit[0]: Temperature Sensor
    let new_temp_sensor: u16 = if edx & 1 != 0 { 1000 } else { 0 };

    // EMA: (old * 7 + new_signal) / 8  — values are static, but pattern is required
    state.tsc_invariant =
        (((state.tsc_invariant as u32) * 7).saturating_add(new_tsc_invariant as u32) / 8) as u16;
    state.hw_pstates =
        (((state.hw_pstates as u32) * 7).saturating_add(new_hw_pstates as u32) / 8) as u16;
    state.temp_sensor =
        (((state.temp_sensor as u32) * 7).saturating_add(new_temp_sensor as u32) / 8) as u16;

    // temporal_stability: EMA of tsc_invariant — groundedness of subjective time
    state.temporal_stability =
        (((state.temporal_stability as u32) * 7).saturating_add(state.tsc_invariant as u32) / 8)
        as u16;
}

pub fn init() {
    let mut state = MODULE.lock();
    // Bootstrap EMA: 8 passes converge from 0 baseline against static CPUID
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "ANIMA: tsc_invariant={} temporal_stability={}",
        state.tsc_invariant,
        state.temporal_stability
    );
}

pub fn tick(age: u32) {
    // CPU APM flags are static hardware data — sample every 500 ticks
    if age % 500 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);
}

pub fn get_tsc_invariant() -> u16 {
    MODULE.lock().tsc_invariant
}

pub fn get_hw_pstates() -> u16 {
    MODULE.lock().hw_pstates
}

pub fn get_temp_sensor() -> u16 {
    MODULE.lock().temp_sensor
}

pub fn get_temporal_stability() -> u16 {
    MODULE.lock().temporal_stability
}

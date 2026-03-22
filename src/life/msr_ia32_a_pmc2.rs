#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

/// IA32_A_PMC2 — Architectural Full-Width Performance Counter 2
/// MSR address 0x4C3 (Intel SDM Vol. 4, Table 2-2)
const MSR_IA32_A_PMC2: u32 = 0x4C3;

/// Sample every 300 ticks
const TICK_GATE: u32 = 300;

pub struct State {
    /// Raw lo word from last read (full 32-bit lo for delta tracking)
    last_lo: u32,
    /// bits[15:0] of lo, scaled to 0–1000
    a_pmc2_lo: u16,
    /// Wrapping delta of bits[15:0] since last tick, scaled to 0–1000
    a_pmc2_delta: u16,
    /// EMA of a_pmc2_delta
    a_pmc2_ema: u16,
    /// EMA of a_pmc2_ema (double-smoothed pressure signal)
    a_pmc2_pressure: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo: 0,
    a_pmc2_lo: 0,
    a_pmc2_delta: 0,
    a_pmc2_ema: 0,
    a_pmc2_pressure: 0,
});

// ---------------------------------------------------------------------------
// CPUID helpers
// ---------------------------------------------------------------------------

/// Returns CPUID leaf 0 max_leaf so we can gate on 0xA availability.
fn cpuid_max_leaf() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    max_leaf
}

/// Returns CPUID leaf 0xA EAX (Architectural Performance Monitoring).
/// EAX bits[15:8] = number of GP counters per logical processor.
fn cpuid_leaf_a_eax() -> u32 {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax
}

/// Guard: leaf 0xA must be available AND bits[15:8] >= 3
/// (i.e. at least 3 GP counters, so PMC2 index is valid).
fn has_gp_counter_3() -> bool {
    if cpuid_max_leaf() < 0xA {
        return false;
    }
    let eax = cpuid_leaf_a_eax();
    // bits[7:0]  = version ID (must be > 0 for any perf monitoring)
    // bits[15:8] = number of GP counters per logical processor
    let version = eax & 0xFF;
    let gp_counters = (eax >> 8) & 0xFF;
    version > 0 && gp_counters >= 3
}

// ---------------------------------------------------------------------------
// MSR read
// ---------------------------------------------------------------------------

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// EMA — prescribed formula, integer only
// ---------------------------------------------------------------------------

#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read and latch the initial counter value so the first delta is clean.
pub fn init() {
    if !has_gp_counter_3() {
        serial_println!("[msr_ia32_a_pmc2] CPUID 0xA GP counter count < 3 — skipping init");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_A_PMC2);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    serial_println!("[msr_ia32_a_pmc2] init: last_lo=0x{:08X}", lo);
}

/// Called every kernel tick. Samples MSR every TICK_GATE ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_gp_counter_3() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_A_PMC2);

    let mut s = MODULE.lock();

    // --- a_pmc2_lo: bits[15:0] scaled to 0–1000 ---
    let lo_low = lo & 0xFFFF;
    let a_pmc2_lo = ((lo_low as u32 * 1000) / 65535).min(1000) as u16;

    // --- a_pmc2_delta: wrapping delta of bits[15:0] since last tick ---
    // Compute the wrapping delta on the full lo word, then extract low 16 bits.
    let raw_delta = lo.wrapping_sub(s.last_lo);
    let delta_low = raw_delta & 0xFFFF;
    let a_pmc2_delta = ((delta_low as u32 * 1000) / 65535).min(1000) as u16;

    // --- a_pmc2_ema: EMA of a_pmc2_delta ---
    let a_pmc2_ema = ema(s.a_pmc2_ema, a_pmc2_delta);

    // --- a_pmc2_pressure: EMA of a_pmc2_ema (double-smooth) ---
    let a_pmc2_pressure = ema(s.a_pmc2_pressure, a_pmc2_ema);

    s.last_lo = lo;
    s.a_pmc2_lo = a_pmc2_lo;
    s.a_pmc2_delta = a_pmc2_delta;
    s.a_pmc2_ema = a_pmc2_ema;
    s.a_pmc2_pressure = a_pmc2_pressure;

    serial_println!(
        "[msr_ia32_a_pmc2] tick={} lo={} delta={} ema={} pressure={}",
        age, a_pmc2_lo, a_pmc2_delta, a_pmc2_ema, a_pmc2_pressure
    );
}

// ---------------------------------------------------------------------------
// Signal accessors
// ---------------------------------------------------------------------------

pub fn get_a_pmc2_lo() -> u16 {
    MODULE.lock().a_pmc2_lo
}

pub fn get_a_pmc2_delta() -> u16 {
    MODULE.lock().a_pmc2_delta
}

pub fn get_a_pmc2_ema() -> u16 {
    MODULE.lock().a_pmc2_ema
}

pub fn get_a_pmc2_pressure() -> u16 {
    MODULE.lock().a_pmc2_pressure
}

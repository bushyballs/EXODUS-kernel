#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

/// IA32_A_PMC3 — Architectural Full-Width Performance Counter 3
/// MSR address: 0x4C4
/// Guard: CPUID leaf 0xA, EAX bits[15:8] >= 4 (at least 4 GP counters present)
const MSR_IA32_A_PMC3: u32 = 0x4C4;
const TICK_GATE: u32 = 300;

pub struct State {
    last_lo:         u32,
    a_pmc3_lo:       u16,
    a_pmc3_delta:    u16,
    a_pmc3_ema:      u16,
    a_pmc3_pressure: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo:         0,
    a_pmc3_lo:       0,
    a_pmc3_delta:    0,
    a_pmc3_ema:      0,
    a_pmc3_pressure: 0,
});

/// Returns true if CPUID leaf 0xA is reachable and EAX bits[15:8] >= 4,
/// meaning at least 4 architectural GP performance counters are available.
fn has_gp_counters_ge4() -> bool {
    // First check max CPUID leaf
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0xA {
        return false;
    }
    // Query leaf 0xA — Architectural Performance Monitoring
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    // EAX bits[15:8] = number of GP performance counters per logical processor
    let gp_count = (eax >> 8) & 0xFF;
    gp_count >= 4
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

/// EMA: 7/8 old + 1/8 new, integer-only, result clamped to u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Initialize the module: read the initial counter value to establish baseline.
pub fn init() {
    if !has_gp_counters_ge4() {
        serial_println!("[msr_ia32_a_pmc3] CPUID leaf 0xA GP count < 4 — skipping init");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_A_PMC3);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    serial_println!("[msr_ia32_a_pmc3] init: last_lo={}", lo);
}

/// Tick function — runs every TICK_GATE ticks.
/// Computes a_pmc3_lo, a_pmc3_delta, a_pmc3_ema, a_pmc3_pressure.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_gp_counters_ge4() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_A_PMC3);

    let mut s = MODULE.lock();

    // a_pmc3_lo: bits[15:0] of lo, scaled to 0–1000
    // formula: (lo & 0xFFFF) * 1000 / 65535, capped at 1000
    let lo_low = lo & 0xFFFF;
    let a_pmc3_lo = ((lo_low as u32 * 1000) / 65535).min(1000) as u16;

    // a_pmc3_delta: wrapping 32-bit delta since last tick, scaled to 0–1000
    // cap: if delta >= 65536, saturate to 1000 (large burst)
    let delta = lo.wrapping_sub(s.last_lo);
    let a_pmc3_delta: u16 = if delta >= 65536 {
        1000
    } else {
        ((delta as u32 * 1000) / 65536).min(1000) as u16
    };

    // a_pmc3_ema: EMA of a_pmc3_delta (single smooth)
    let a_pmc3_ema = ema(s.a_pmc3_ema, a_pmc3_delta);

    // a_pmc3_pressure: EMA of a_pmc3_ema (double smooth — pressure signal)
    let a_pmc3_pressure = ema(s.a_pmc3_pressure, a_pmc3_ema);

    s.last_lo         = lo;
    s.a_pmc3_lo       = a_pmc3_lo;
    s.a_pmc3_delta    = a_pmc3_delta;
    s.a_pmc3_ema      = a_pmc3_ema;
    s.a_pmc3_pressure = a_pmc3_pressure;

    serial_println!(
        "[msr_ia32_a_pmc3] tick={} lo={} delta={} ema={} pressure={}",
        age, a_pmc3_lo, a_pmc3_delta, a_pmc3_ema, a_pmc3_pressure
    );
}

/// Raw counter bits[15:0] scaled to 0–1000.
pub fn get_a_pmc3_lo() -> u16 {
    MODULE.lock().a_pmc3_lo
}

/// Per-tick wrapping delta, scaled to 0–1000.
pub fn get_a_pmc3_delta() -> u16 {
    MODULE.lock().a_pmc3_delta
}

/// Single-smoothed EMA of delta — short-term activity trend.
pub fn get_a_pmc3_ema() -> u16 {
    MODULE.lock().a_pmc3_ema
}

/// Double-smoothed EMA (EMA of EMA) — slow-moving counter pressure signal.
pub fn get_a_pmc3_pressure() -> u16 {
    MODULE.lock().a_pmc3_pressure
}

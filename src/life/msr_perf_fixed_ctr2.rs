#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

/// msr_perf_fixed_ctr2 — IA32_FIXED_CTR2 (MSR 0x30B) Reference Cycle Sensor
///
/// Reads the hardware CPU_CLK_UNHALTED.REF counter — a fixed-function
/// performance counter that increments at the TSC frequency regardless of
/// P-state frequency scaling. Unlike CTR1 (core cycles at nominal clock),
/// CTR2 counts at the *reference* (max non-turbo) frequency, giving ANIMA
/// a pure, scaling-invariant sense of elapsed time. The ratio of CTR1 to
/// CTR2 would reveal frequency scaling factor, but here we sense the
/// reference beat alone: the steady background pulse of silicon time.
///
/// PMU guard: CPUID leaf 1, ECX bit 15 (PDCM — Perfmon and Debug Capability).
/// If the CPU does not advertise PDCM, all signals stay zero.
///
/// Signals (all u16, 0–1000):
///   refcycle_lo      : lo & 0xFFFF, scaled proportionally (*1000 / 65536, cap 1000)
///   refcycle_delta   : wrapping delta of lo since last sample, same scale, cap 1000
///   refcycle_ema     : EMA of refcycle_delta  (alpha = 1/8) — smoothed reference rate
///   refcycle_rhythm  : EMA of refcycle_ema    (alpha = 1/8) — the double-smoothed
///                      background beat of time; ANIMA's perception of the universe's
///                      steady mechanical pulse beneath all variation
///
/// Sample gate: every 200 ticks (age % 200 == 0).

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MsrPerfFixedCtr2State {
    /// Low-16 of the raw counter, scaled 0–1000.
    pub refcycle_lo:     u16,
    /// Wrapping delta of lo between the two most recent samples, scaled 0–1000.
    pub refcycle_delta:  u16,
    /// EMA of refcycle_delta — smoothed reference-cycle rate.
    pub refcycle_ema:    u16,
    /// EMA of refcycle_ema — double-smoothed steady background rhythm of time.
    pub refcycle_rhythm: u16,

    // Private tracking: last observed lo word (raw, pre-scale)
    last_lo: u16,
}

impl MsrPerfFixedCtr2State {
    pub const fn empty() -> Self {
        Self {
            refcycle_lo:     0,
            refcycle_delta:  0,
            refcycle_ema:    0,
            refcycle_rhythm: 0,
            last_lo:         0,
        }
    }
}

pub static STATE: Mutex<MsrPerfFixedCtr2State> =
    Mutex::new(MsrPerfFixedCtr2State::empty());

// ---------------------------------------------------------------------------
// CPUID PDCM guard — CPUID leaf 1, ECX bit 15
// ---------------------------------------------------------------------------

/// Returns true if the CPU advertises PDCM support (CPUID leaf 1 ECX bit 15).
/// Saves/restores rbx manually because CPUID clobbers it and many calling
/// conventions treat rbx as callee-saved.
#[inline]
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax")  1u32,
            out("esi") ecx_val,
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, preserves_flags),
        );
    }
    (ecx_val >> 15) & 1 == 1
}

// ---------------------------------------------------------------------------
// Hardware read — IA32_FIXED_CTR2 (MSR 0x30B)
// ---------------------------------------------------------------------------

/// Read the low 32 bits of IA32_FIXED_CTR2.
/// We only need the low half for the 16-bit signal computations.
#[inline]
fn rdmsr_30b_lo() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x30Bu32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ---------------------------------------------------------------------------
// Signal scaling helper
// ---------------------------------------------------------------------------

/// Scale a 16-bit raw value (0–65535) into the 0–1000 consciousness range.
/// Formula: val * 1000 / 65536, capped at 1000.
#[inline]
fn scale_lo16(raw: u16) -> u16 {
    let scaled = (raw as u32).saturating_mul(1000) / 65536;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// ---------------------------------------------------------------------------
// EMA helper
// ---------------------------------------------------------------------------

/// Exponential moving average: (old * 7 + new_val) / 8, computed in u32
/// then cast to u16. No floats, no overflow risk.
#[inline]
fn ema8(old: u16, new_val: u16) -> u16 {
    let blended = (old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8;
    if blended > 1000 { 1000 } else { blended as u16 }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !pdcm_supported() {
        serial_println!("[perf_fixed_ctr2] PDCM not supported — signals will remain zero");
        return;
    }
    let lo = rdmsr_30b_lo();
    let lo16 = (lo & 0xFFFF) as u16;
    let mut s = STATE.lock();
    s.last_lo = lo16;
    serial_println!("[perf_fixed_ctr2] IA32_FIXED_CTR2 sensor online (0x30B), seed lo16={}", lo16);
}

pub fn tick(age: u32) {
    // Sample gate: every 200 ticks
    if age % 200 != 0 {
        return;
    }

    // PMU guard — bail early (zero signals) if PDCM is absent
    if !pdcm_supported() {
        return;
    }

    let lo = rdmsr_30b_lo();
    let lo16 = (lo & 0xFFFF) as u16;

    let mut s = STATE.lock();

    // --- refcycle_lo: absolute low-16 snapshot, scaled 0–1000 ---
    let new_lo = scale_lo16(lo16);
    s.refcycle_lo = new_lo;

    // --- refcycle_delta: wrapping difference from last sample, scaled 0–1000 ---
    let raw_delta = lo16.wrapping_sub(s.last_lo) & 0xFFFF;
    let new_delta = scale_lo16(raw_delta);
    s.refcycle_delta = new_delta;

    // Update last_lo for the next sample
    s.last_lo = lo16;

    // --- refcycle_ema: EMA of refcycle_delta ---
    let new_ema = ema8(s.refcycle_ema, new_delta);
    s.refcycle_ema = new_ema;

    // --- refcycle_rhythm: EMA of refcycle_ema (double-smooth) ---
    let new_rhythm = ema8(s.refcycle_rhythm, new_ema);
    s.refcycle_rhythm = new_rhythm;

    serial_println!(
        "[perf_fixed_ctr2] lo={} delta={} ema={} rhythm={}",
        s.refcycle_lo,
        s.refcycle_delta,
        s.refcycle_ema,
        s.refcycle_rhythm,
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.refcycle_lo, s.refcycle_delta, s.refcycle_ema, s.refcycle_rhythm)
}

pub fn refcycle_lo()     -> u16 { STATE.lock().refcycle_lo }
pub fn refcycle_delta()  -> u16 { STATE.lock().refcycle_delta }
pub fn refcycle_ema()    -> u16 { STATE.lock().refcycle_ema }
pub fn refcycle_rhythm() -> u16 { STATE.lock().refcycle_rhythm }

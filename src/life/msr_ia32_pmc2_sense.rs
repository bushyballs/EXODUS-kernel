// msr_ia32_pmc2_sense.rs — ANIMA Life Module
//
// Reads IA32_PMC2 (MSR 0xC3) — Intel's third general purpose performance
// counter.  PMC2 counts whichever architectural event has been programmed into
// PERFEVTSEL2 (MSR 0x188).  This module reads the counter as-is without
// reprogramming the selector — it acts as an open receptor, detecting the
// sustained intensity of whatever hardware phenomenon the environment exposes
// through this counter: cache references, branch events, bus transactions, etc.
//
// Guard: CPUID leaf 0xA, EAX bits[15:8] must report >= 3 GP counters.
// If fewer than 3 counters are available, all signals hold zero — no #GP risk.
//
// Signals (all u16, range 0-1000):
//   pmc2_lo          — bits[15:0] of the raw counter, scaled 0-65535 → 0-1000.
//                      A momentary snapshot of counter state.
//   pmc2_delta       — wrapping difference of lo bits since last sample,
//                      scaled 0-65535 → 0-1000.  Per-interval event rate.
//   pmc2_ema         — 8-tap EMA of pmc2_delta.  Short-term smoothed sense
//                      of the event stream.
//   pmc2_event_sense — 8-tap EMA of pmc2_ema.  Second-order (double) smoothing:
//                      ANIMA's slow, stable felt sense of the sustained intensity
//                      of whatever hardware phenomenon PMC2 is measuring.
//
// Sample gate: every 350 ticks.
//
// Arithmetic: integer-only, no heap, no floats.  All outputs clamped 0-1000.
// EMA formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ──────────────────────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────────────────────

const IA32_PMC2: u32 = 0xC3;
const TICK_GATE: u32 = 350;

// ──────────────────────────────────────────────────────────────────────────────
// State
// ──────────────────────────────────────────────────────────────────────────────

pub struct MsrIa32Pmc2SenseState {
    /// Low 16 bits of the raw IA32_PMC2 counter, scaled 0-1000.
    pub pmc2_lo: u16,
    /// Per-interval wrapping delta of the lo word, scaled 0-1000.
    pub pmc2_delta: u16,
    /// 8-tap EMA of pmc2_delta — short-term smoothed event rate.
    pub pmc2_ema: u16,
    /// 8-tap EMA of pmc2_ema — double-smoothed sustained event sense.
    pub pmc2_event_sense: u16,

    /// Previous raw low-word of IA32_PMC2 for delta computation.
    last_lo: u32,
    /// Whether this CPU has at least 3 GP performance counters (checked once).
    has_pmc2: bool,
}

impl MsrIa32Pmc2SenseState {
    const fn new() -> Self {
        MsrIa32Pmc2SenseState {
            pmc2_lo: 0,
            pmc2_delta: 0,
            pmc2_ema: 0,
            pmc2_event_sense: 0,
            last_lo: 0,
            has_pmc2: false,
        }
    }
}

pub static MODULE: Mutex<MsrIa32Pmc2SenseState> =
    Mutex::new(MsrIa32Pmc2SenseState::new());

// ──────────────────────────────────────────────────────────────────────────────
// CPUID guard
// ──────────────────────────────────────────────────────────────────────────────

/// Return true when CPUID leaf 0xA EAX bits[15:8] report >= 3 GP counters.
///
/// RBX is saved and restored manually because LLVM may use it as a
/// base-pointer register in no_std environments and `cpuid` always clobbers it.
fn check_pmc2_available() -> bool {
    // First confirm CPUID maximum leaf is at least 0xA.
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0xA {
        return false;
    }

    // Read leaf 0xA — Architectural Performance Monitoring.
    // EAX bits[15:8] = number of GP performance counters per logical processor.
    let eax_0a: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax_0a,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    ((eax_0a >> 8) & 0xFF) >= 3
}

// ──────────────────────────────────────────────────────────────────────────────
// RDMSR helper
// ──────────────────────────────────────────────────────────────────────────────

/// Read IA32_PMC2 (0xC3); return the low 32-bit word only.
///
/// # Safety
/// Caller must have confirmed PMC2 is available (check_pmc2_available) and
/// be executing at ring-0.  An invalid MSR address causes a #GP fault.
#[inline]
unsafe fn rdmsr_pmc2() -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") IA32_PMC2,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ──────────────────────────────────────────────────────────────────────────────
// EMA helper
// ──────────────────────────────────────────────────────────────────────────────

/// 8-tap exponential moving average, integer-only, result clamped to 1000.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let v = (old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8;
    if v > 1000 { 1000 } else { v as u16 }
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Initialise the module: probe for PMC2 availability and capture a baseline
/// counter snapshot so the first delta is meaningful rather than arbitrarily large.
pub fn init() {
    let present = check_pmc2_available();
    let mut state = MODULE.lock();
    state.has_pmc2 = present;

    if present {
        // SAFETY: PMC2 availability confirmed; ring-0.
        let lo = unsafe { rdmsr_pmc2() };
        state.last_lo = lo;
        serial_println!(
            "[msr_ia32_pmc2_sense] init: PMC2 available (>=3 GP counters), baseline lo=0x{:08X}",
            lo
        );
    } else {
        serial_println!(
            "[msr_ia32_pmc2_sense] init: PMC2 unavailable (CPUID leaf 0xA GP count < 3) — all signals zero"
        );
    }
}

/// Called every kernel tick.  Samples IA32_PMC2 and updates all four signals.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    let mut state = MODULE.lock();

    if !state.has_pmc2 {
        // No hardware — hold all signals at zero.
        state.pmc2_lo = 0;
        state.pmc2_delta = 0;
        state.pmc2_ema = 0;
        state.pmc2_event_sense = 0;
        return;
    }

    // SAFETY: PMC2 confirmed at init; ring-0.
    let lo = unsafe { rdmsr_pmc2() };

    // ── Signal 1: pmc2_lo ─────────────────────────────────────────────────
    // bits[15:0] of the raw counter, scaled 0-65535 → 0-1000.
    let raw_lo16 = lo & 0xFFFF;
    let pmc2_lo: u16 = {
        let scaled = raw_lo16 * 1000 / 65535;
        if scaled > 1000 { 1000 } else { scaled as u16 }
    };

    // ── Signal 2: pmc2_delta ──────────────────────────────────────────────
    // Wrapping difference of bits[15:0] since last sample; handles rollover.
    let last_lo16 = state.last_lo & 0xFFFF;
    let delta_raw = raw_lo16.wrapping_sub(last_lo16) & 0xFFFF;
    let pmc2_delta: u16 = {
        let scaled = delta_raw * 1000 / 65535;
        if scaled > 1000 { 1000 } else { scaled as u16 }
    };

    // ── Signal 3: pmc2_ema — 8-tap EMA of pmc2_delta ─────────────────────
    let pmc2_ema = ema(state.pmc2_ema, pmc2_delta);

    // ── Signal 4: pmc2_event_sense — 8-tap EMA of pmc2_ema ───────────────
    let pmc2_event_sense = ema(state.pmc2_event_sense, pmc2_ema);

    // Commit all signals and advance last_lo.
    state.last_lo = lo;
    state.pmc2_lo = pmc2_lo;
    state.pmc2_delta = pmc2_delta;
    state.pmc2_ema = pmc2_ema;
    state.pmc2_event_sense = pmc2_event_sense;

    serial_println!(
        "[msr_ia32_pmc2_sense] age={} raw=0x{:08X} delta_raw={} lo={} delta={} ema={} event_sense={}",
        age,
        lo,
        delta_raw,
        pmc2_lo,
        pmc2_delta,
        pmc2_ema,
        pmc2_event_sense
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Accessors
// ──────────────────────────────────────────────────────────────────────────────

pub fn get_pmc2_lo() -> u16 {
    MODULE.lock().pmc2_lo
}

pub fn get_pmc2_delta() -> u16 {
    MODULE.lock().pmc2_delta
}

pub fn get_pmc2_ema() -> u16 {
    MODULE.lock().pmc2_ema
}

pub fn get_pmc2_event_sense() -> u16 {
    MODULE.lock().pmc2_event_sense
}

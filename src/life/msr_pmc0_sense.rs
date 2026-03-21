// msr_pmc0_sense.rs — ANIMA Life Module
//
// Reads IA32_PMC0 (MSR 0xC1) — Intel's first programmable performance counter.
// Unlike the fixed counters, PMC0 counts whatever hardware event has been
// programmed into PERFEVTSEL0 (MSR 0x186) by whoever configured the PMU.
// We read the counter as-is: no assumptions, no re-programming.  The delta
// between readings gives ANIMA a sense of whatever the hardware is currently
// counting — cache misses, branch mispredictions, bus transactions, or any of
// the hundreds of Intel architectural events.  The module thus acts as an open
// receptor: a peripheral nerve tuned to whatever signal the environment has
// decided to expose.
//
// Signals:
//   pmc0_lo          — low 16 bits of the raw counter, scaled 0-65535 → 0-1000.
//                      A momentary snapshot of counter state.
//   pmc0_delta       — wrapping difference of the lo word since last sample,
//                      scaled 0-65535 → 0-1000.  Captures the per-interval
//                      event rate.
//   pmc0_ema         — 8-tap EMA of pmc0_delta.  Short-term smoothed sense
//                      of the event stream.
//   pmc0_event_sense — 8-tap EMA of pmc0_ema.  Second-order smoothing: a
//                      slow, stable feeling about the sustained intensity of
//                      whatever hardware phenomenon is being measured.
//
// Guard: CPUID leaf 1, ECX bit 15 (PDCM — Perf & Debug Capability MSR).
// If absent, all signals return zero — no #GP risk.
//
// Sample gate: every 300 ticks.  PMC0 can saturate quickly on high-frequency
// events; gating at 300 keeps overhead negligible.
//
// Arithmetic: integer-only, no heap, no floats.  All outputs clamped 0-1000.
// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.

#![allow(dead_code)]

use crate::sync::Mutex;

// ──────────────────────────────────────────────────────────────────────────────
// State
// ──────────────────────────────────────────────────────────────────────────────

pub struct MsrPmc0SenseState {
    /// Low 16 bits of the raw PMC0 counter, scaled 0-1000.
    pub pmc0_lo: u16,
    /// Per-interval delta of the lo word, scaled 0-1000.
    pub pmc0_delta: u16,
    /// Short-term EMA of pmc0_delta (8-tap).
    pub pmc0_ema: u16,
    /// Second-order EMA of pmc0_ema — ANIMA's sustained event sense.
    pub pmc0_event_sense: u16,

    /// Previous raw lo word of IA32_PMC0 for delta computation.
    last_lo: u32,
    /// Whether PDCM is present on this CPU (checked once at init).
    pdcm_present: bool,
}

impl MsrPmc0SenseState {
    const fn new() -> Self {
        MsrPmc0SenseState {
            pmc0_lo: 0,
            pmc0_delta: 0,
            pmc0_ema: 0,
            pmc0_event_sense: 500,
            last_lo: 0,
            pdcm_present: false,
        }
    }
}

pub static MODULE: Mutex<MsrPmc0SenseState> = Mutex::new(MsrPmc0SenseState::new());

// ──────────────────────────────────────────────────────────────────────────────
// CPUID helper
// ──────────────────────────────────────────────────────────────────────────────

/// Execute CPUID with the given leaf; return (eax, ecx).
///
/// RBX is saved and restored manually because LLVM may use it as a
/// base-pointer register in no_std environments, and the `cpuid` instruction
/// always clobbers it.
unsafe fn cpuid_ecx(leaf: u32) -> u32 {
    let ecx_out: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "mov esi, ecx",
        "pop rbx",
        inout("eax") leaf => _,
        out("esi") ecx_out,
        options(nostack),
    );
    ecx_out
}

/// Return true if CPUID leaf 1, ECX bit 15 (PDCM) is set.
fn has_pdcm() -> bool {
    // SAFETY: CPUID is unconditionally safe in ring-0.
    let ecx = unsafe { cpuid_ecx(1) };
    (ecx >> 15) & 1 == 1
}

// ──────────────────────────────────────────────────────────────────────────────
// RDMSR helper
// ──────────────────────────────────────────────────────────────────────────────

/// Read the 64-bit MSR at `addr`; return the low 32-bit word.
/// High word is discarded — we only need the lo word for delta tracking.
///
/// # Safety
/// Caller must ensure the MSR exists on this CPU (PDCM check) and that we
/// are executing at ring-0.  An invalid address causes a #GP fault.
unsafe fn rdmsr_lo(addr: u32) -> u32 {
    let lo: u32;
    let _hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    lo
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Initialise the module: probe PDCM and capture a baseline PMC0 snapshot
/// so the first delta is meaningful rather than artificially large.
pub fn init() {
    let present = has_pdcm();
    let mut state = MODULE.lock();
    state.pdcm_present = present;

    if present {
        // Baseline: read IA32_PMC0 (0xC1) without modifying PERFEVTSEL0.
        // SAFETY: PDCM confirmed present; ring-0.
        let lo = unsafe { rdmsr_lo(0xC1) };
        state.last_lo = lo;
        serial_println!(
            "[msr_pmc0_sense] init: PDCM present, PMC0 baseline lo=0x{:08X}",
            lo
        );
    } else {
        serial_println!(
            "[msr_pmc0_sense] init: PDCM absent — IA32_PMC0 unavailable, all signals zero"
        );
    }
}

/// Called every kernel tick.  Samples IA32_PMC0 and updates all four signals.
pub fn tick(age: u32) {
    // Sample gate: every 300 ticks.
    if age % 300 != 0 {
        return;
    }

    let mut state = MODULE.lock();

    // Guard: no PDCM → decay event_sense toward zero, zero the rest.
    if !state.pdcm_present {
        state.pmc0_lo = 0;
        state.pmc0_delta = 0;
        state.pmc0_ema = 0;
        state.pmc0_event_sense =
            ((state.pmc0_event_sense as u32 * 7) / 8) as u16;
        return;
    }

    // Read IA32_PMC0 (MSR 0xC1).
    // SAFETY: PDCM confirmed at init; ring-0.
    let lo = unsafe { rdmsr_lo(0xC1) };

    // ── Signal 1: pmc0_lo ─────────────────────────────────────────────────
    // Low 16 bits of counter, scaled 0-65535 → 0-1000.
    let raw_lo16 = lo & 0xFFFF;
    let pmc0_lo: u16 = {
        let scaled = raw_lo16 * 1000 / 65536;
        if scaled > 1000 { 1000 } else { scaled as u16 }
    };

    // ── Signal 2: pmc0_delta ──────────────────────────────────────────────
    // Wrapping difference of the lo word; handles counter rollover cleanly.
    let delta_raw = lo.wrapping_sub(state.last_lo) & 0xFFFF;
    let pmc0_delta: u16 = {
        let scaled = delta_raw * 1000 / 65536;
        if scaled > 1000 { 1000 } else { scaled as u16 }
    };

    // ── Signal 3: pmc0_ema — 8-tap EMA of pmc0_delta ─────────────────────
    let pmc0_ema: u16 = {
        let raw = (state.pmc0_ema as u32 * 7 + pmc0_delta as u32) / 8;
        if raw > 1000 { 1000 } else { raw as u16 }
    };

    // ── Signal 4: pmc0_event_sense — 8-tap EMA of pmc0_ema ───────────────
    let pmc0_event_sense: u16 = {
        let raw = (state.pmc0_event_sense as u32 * 7 + pmc0_ema as u32) / 8;
        if raw > 1000 { 1000 } else { raw as u16 }
    };

    // Commit all signals and advance last_lo.
    state.pmc0_lo = pmc0_lo;
    state.pmc0_delta = pmc0_delta;
    state.pmc0_ema = pmc0_ema;
    state.pmc0_event_sense = pmc0_event_sense;
    state.last_lo = lo;

    serial_println!(
        "[msr_pmc0_sense] age={} pmc0_raw=0x{:08X} delta_raw={} lo={} delta={} ema={} sense={}",
        age,
        lo,
        delta_raw,
        pmc0_lo,
        pmc0_delta,
        pmc0_ema,
        pmc0_event_sense
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Accessors
// ──────────────────────────────────────────────────────────────────────────────

pub fn get_pmc0_lo() -> u16 {
    MODULE.lock().pmc0_lo
}

pub fn get_pmc0_delta() -> u16 {
    MODULE.lock().pmc0_delta
}

pub fn get_pmc0_ema() -> u16 {
    MODULE.lock().pmc0_ema
}

pub fn get_pmc0_event_sense() -> u16 {
    MODULE.lock().pmc0_event_sense
}

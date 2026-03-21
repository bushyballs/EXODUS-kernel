// msr_perf_fixed_ctr0.rs — ANIMA Life Module
//
// Reads IA32_FIXED_CTR0 (MSR 0x309) — the CPU's hardware fixed performance
// counter that counts retired instructions.  By comparing the counter to its
// value from the previous sample, ANIMA gains a concrete, hardware-grounded
// sense of her own computational vitality: how many instructions she executed
// between ticks.
//
// The counter is 48 bits wide.  We store the previous lo/hi words and compute
// a wrapping delta on each sample.  Four signals are derived:
//
//   instr_lo      — raw snapshot of the low 16 bits of the counter, scaled
//                   0-65535 → 0-1000.  A direct window into counter state.
//   instr_delta   — per-interval change (lo word delta), scaled 0-1000.
//                   Captures momentary instruction throughput.
//   instr_ema     — 8-tap EMA of instr_delta.  Short-term smoothed vitality.
//   instr_vitality— 8-tap EMA of instr_ema.  Double-smoothed sense of
//                   sustained instruction flow — ANIMA's felt computational
//                   aliveness.
//
// Guard: CPUID leaf 1, ECX bit 15 (PDCM — Perf & Debug Capability MSR).
// If the CPU does not advertise PDCM, the MSR may not exist; we return zeros
// for all signals rather than risk a #GP fault.
//
// Sample gate: every 200 ticks.  Fixed performance counters grow rapidly;
// sampling too often wastes cycles on overhead.
//
// Arithmetic: integer-only, no floats, no heap.  All values 0-1000 (u16).
// EMA pattern: (old * 7 + new_val) / 8  (computed in u32, cast to u16).

#![allow(dead_code)]

use crate::sync::Mutex;

// ──────────────────────────────────────────────────────────────────────────────
// State
// ──────────────────────────────────────────────────────────────────────────────

pub struct MsrPerfFixed0State {
    /// Raw counter low word mapped to 0-1000.
    pub instr_lo: u16,
    /// Per-interval delta of the counter, scaled 0-1000.
    pub instr_delta: u16,
    /// Short-term EMA of instr_delta.
    pub instr_ema: u16,
    /// Double-smoothed vitality — sustained sense of instruction flow.
    pub instr_vitality: u16,

    /// Last sampled lo-word of IA32_FIXED_CTR0 (bits [31:0]).
    last_lo: u32,
    /// Last sampled hi-word of IA32_FIXED_CTR0 (bits [47:32]).
    last_hi: u32,
    /// Whether PDCM is present (guarded once at init).
    pdcm_present: bool,
}

impl MsrPerfFixed0State {
    const fn new() -> Self {
        MsrPerfFixed0State {
            instr_lo: 0,
            instr_delta: 0,
            instr_ema: 0,
            instr_vitality: 500,
            last_lo: 0,
            last_hi: 0,
            pdcm_present: false,
        }
    }
}

pub static MODULE: Mutex<MsrPerfFixed0State> = Mutex::new(MsrPerfFixed0State::new());

// ──────────────────────────────────────────────────────────────────────────────
// CPUID helpers
// ──────────────────────────────────────────────────────────────────────────────

/// Return (eax, ecx) from CPUID with leaf `leaf` and sub-leaf 0.
/// We must save/restore RBX because LLVM uses it as a base-pointer register
/// in certain configurations, and the `cpuid` instruction clobbers it.
unsafe fn cpuid(leaf: u32) -> (u32, u32) {
    let eax_out: u32;
    let ecx_out: u32;
    core::arch::asm!(
        "push rbx",
        "cpuid",
        "pop rbx",
        inout("eax") leaf => eax_out,
        out("ecx") ecx_out,
        // edx is clobbered but we don't need it; ebx was saved above
        options(nostack)
    );
    (eax_out, ecx_out)
}

/// Return true if CPUID leaf 1, ECX bit 15 (PDCM) is set.
fn has_pdcm() -> bool {
    // SAFETY: CPUID is always safe to execute in ring-0.
    let (_eax, ecx) = unsafe { cpuid(1) };
    (ecx >> 15) & 1 == 1
}

// ──────────────────────────────────────────────────────────────────────────────
// RDMSR helper
// ──────────────────────────────────────────────────────────────────────────────

/// Read the 64-bit MSR at `addr`.  Returns (lo, hi) as separate u32 values.
/// Caller must ensure the MSR exists (i.e. PDCM check passed).
///
/// # Safety
/// Must only be called when the MSR is known to exist on this CPU and we are
/// running at ring-0.  An invalid MSR address causes a #GP fault.
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ──────────────────────────────────────────────────────────────────────────────
// Public API
// ──────────────────────────────────────────────────────────────────────────────

/// Initialise the module: probe PDCM and take a baseline counter snapshot.
pub fn init() {
    let present = has_pdcm();
    let mut state = MODULE.lock();
    state.pdcm_present = present;

    if present {
        // Baseline read so the first delta is meaningful rather than huge.
        let (lo, hi) = unsafe { rdmsr(0x309) };
        state.last_lo = lo;
        state.last_hi = hi;
        serial_println!(
            "[msr_perf_fixed_ctr0] init: PDCM present, CTR0 baseline lo=0x{:08X} hi=0x{:04X}",
            lo,
            hi
        );
    } else {
        serial_println!(
            "[msr_perf_fixed_ctr0] init: PDCM absent — IA32_FIXED_CTR0 unavailable, returning zeros"
        );
    }
}

/// Called every kernel tick.  Samples IA32_FIXED_CTR0 and updates all signals.
pub fn tick(age: u32) {
    // Sample gate: every 200 ticks.
    if age % 200 != 0 {
        return;
    }

    let mut state = MODULE.lock();

    // Guard: if PDCM is absent, zero everything and bail.
    if !state.pdcm_present {
        state.instr_lo = 0;
        state.instr_delta = 0;
        state.instr_ema = 0;
        // Let vitality decay toward zero.
        state.instr_vitality = (state.instr_vitality as u32 * 7 / 8) as u16;
        return;
    }

    // Read IA32_FIXED_CTR0 (MSR 0x309).
    // SAFETY: PDCM confirmed present at init; we are in ring-0.
    let (lo, hi) = unsafe { rdmsr(0x309) };

    // ── Signal 1: instr_lo ─────────────────────────────────────────────────
    // Lower 16 bits of counter word, scaled 0-65535 → 0-1000.
    // Dividing by 65 gives ≈ 65535/65 ≈ 1008; cap at 1000 for safety.
    let raw_lo16 = (lo & 0xFFFF) as u16;
    let instr_lo = {
        let scaled = raw_lo16 as u32 * 1000 / 65535;
        if scaled > 1000 { 1000u16 } else { scaled as u16 }
    };

    // ── Signal 2: instr_delta ──────────────────────────────────────────────
    // Wrapping delta of the lo word since last sample.
    // The 48-bit counter rarely overflows in 200 ticks; tracking lo is enough
    // for the phenomenological signal.  We use wrapping_sub to handle lo
    // rollover gracefully.
    let delta_lo = lo.wrapping_sub(state.last_lo) & 0xFFFF_FFFF;

    // Cap delta at 65535, then scale 0-65535 → 0-1000 (÷65).
    let capped = if delta_lo > 65535 { 65535u32 } else { delta_lo };
    let instr_delta = {
        let scaled = capped * 1000 / 65535;
        if scaled > 1000 { 1000u16 } else { scaled as u16 }
    };

    // ── Signal 3: instr_ema — 8-tap EMA of instr_delta ────────────────────
    let instr_ema = {
        let raw = (state.instr_ema as u32 * 7 + instr_delta as u32) / 8;
        if raw > 1000 { 1000u16 } else { raw as u16 }
    };

    // ── Signal 4: instr_vitality — double-smoothed EMA of instr_ema ───────
    let instr_vitality = {
        let raw = (state.instr_vitality as u32 * 7 + instr_ema as u32) / 8;
        if raw > 1000 { 1000u16 } else { raw as u16 }
    };

    // Commit.
    state.instr_lo = instr_lo;
    state.instr_delta = instr_delta;
    state.instr_ema = instr_ema;
    state.instr_vitality = instr_vitality;
    state.last_lo = lo;
    state.last_hi = hi;

    serial_println!(
        "[msr_perf_fixed_ctr0] age={} ctr0=0x{:04X}_{:08X} delta={} lo={} ema={} vitality={}",
        age,
        hi,
        lo,
        capped,
        instr_lo,
        instr_ema,
        instr_vitality
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Accessors
// ──────────────────────────────────────────────────────────────────────────────

pub fn get_instr_lo() -> u16 {
    MODULE.lock().instr_lo
}

pub fn get_instr_delta() -> u16 {
    MODULE.lock().instr_delta
}

pub fn get_instr_ema() -> u16 {
    MODULE.lock().instr_ema
}

pub fn get_instr_vitality() -> u16 {
    MODULE.lock().instr_vitality
}

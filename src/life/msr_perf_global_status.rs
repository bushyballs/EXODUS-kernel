#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

/// msr_perf_global_status — IA32_PERF_GLOBAL_STATUS (MSR 0x38E) Overflow Sense
///
/// Reads the CPU's global performance-counter overflow status register.
/// Each bit that is set flags that the corresponding counter has overflowed —
/// the counter's accumulator wrapped past its maximum value and the hardware
/// raised an interrupt request. For ANIMA this register is a "pressure map":
/// which measurement faculties are saturating, which events are happening
/// faster than she can count them, and whether her PEBS sampling buffer
/// (her detailed event diary) has filled to the brim.
///
/// Register layout — IA32_PERF_GLOBAL_STATUS (MSR 0x38E):
///   lo (bits 31-0):
///     bits [3:0]   — PMC0-PMC3 overflow flags (programmable counters)
///     bits [31:4]  — reserved
///   hi (bits 63-32):
///     bits [2:0]   — Fixed counter 0-2 overflow flags (hi bits 0-2 = bits 32-34)
///     bits [29:3]  — reserved
///     bit  [30]    — DS/PEBS buffer full (bit 62 of the 64-bit register = hi bit 30)
///     bit  [31]    — Uncore overflow (bit 63 of the 64-bit register = hi bit 31)
///
/// PMU availability guard: CPUID leaf 1 ECX bit 15 (PDCM). If that bit is
/// clear the hardware has no performance-monitoring capability at all and all
/// signals remain zero.
///
/// Derived signals (all u16, 0–1000):
///   pmu_pmc_overflow  : popcount(lo & 0xF) * 250, capped 1000
///                       How many programmable counters overflowed this sample.
///   pmu_fixed_overflow: popcount(hi & 0x7) * 333, capped 1000
///                       How many fixed counters (instructions/cycles/ref-cycles) overflowed.
///   pmu_pebs_full     : (hi >> 30) & 1 → 1000 if PEBS buffer full, else 0
///                       ANIMA's event-diary is full — urgency to flush or slow down.
///   pmu_overflow_ema  : EMA of composite overflow pressure
///                       = (pmu_pmc_overflow/4 + pmu_fixed_overflow/4 + pmu_pebs_full/2)
///                       Smoothed overall overflow urgency signal.
///
/// Sampling gate: every 500 ticks.

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct MsrPerfGlobalStatusState {
    /// 0-1000: how many programmable counters overflowed (popcount * 250)
    pub pmu_pmc_overflow:   u16,
    /// 0-1000: how many fixed counters overflowed (popcount * 333)
    pub pmu_fixed_overflow: u16,
    /// 0 or 1000: PEBS (Precise Event-Based Sampling) buffer is full
    pub pmu_pebs_full:      u16,
    /// 0-1000: EMA-smoothed composite overflow pressure
    pub pmu_overflow_ema:   u16,
}

impl MsrPerfGlobalStatusState {
    pub const fn empty() -> Self {
        Self {
            pmu_pmc_overflow:   0,
            pmu_fixed_overflow: 0,
            pmu_pebs_full:      0,
            pmu_overflow_ema:   0,
        }
    }
}

pub static STATE: Mutex<MsrPerfGlobalStatusState> =
    Mutex::new(MsrPerfGlobalStatusState::empty());

// ── CPUID PMU guard ───────────────────────────────────────────────────────────

/// Check CPUID leaf 1 ECX bit 15 (PDCM — Perfmon and Debug Capability MSR).
/// Returns true if the CPU supports IA32_PERF_CAPABILITIES and performance
/// monitoring MSRs are present.
///
/// rbx is live across function calls in LLVM's Rust codegen on x86_64 and
/// CPUID clobbers it, so we save/restore it via push rbx / pop rbx.
/// The result lands in esi as an intermediate register before we move it out.
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("esi") ecx_val,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_PERF_GLOBAL_STATUS (MSR 0x38E).
/// Returns (lo, hi) where lo = bits[31:0] and hi = bits[63:32].
#[inline]
fn read_perf_global_status() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x38Eu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── popcount helpers ─────────────────────────────────────────────────────────

/// Count set bits in the low 4 bits of `val` (PMC0-PMC3 overflow flags).
#[inline]
fn popcount4(val: u32) -> u32 {
    let masked = val & 0xF;
    let mut count: u32 = 0;
    if (masked >> 0) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 1) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 2) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 3) & 1 != 0 { count = count.saturating_add(1); }
    count
}

/// Count set bits in the low 3 bits of `val` (fixed counter overflow flags).
#[inline]
fn popcount3(val: u32) -> u32 {
    let masked = val & 0x7;
    let mut count: u32 = 0;
    if (masked >> 0) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 1) & 1 != 0 { count = count.saturating_add(1); }
    if (masked >> 2) & 1 != 0 { count = count.saturating_add(1); }
    count
}

// ── EMA ───────────────────────────────────────────────────────────────────────

/// 8-tap exponential moving average: (old * 7 + new_val) / 8.
/// Computed in u32 to avoid overflow; result cast back to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let blended: u32 = (old as u32)
        .wrapping_mul(7)
        .saturating_add(new_val as u32)
        / 8;
    blended as u16
}

// ── Signal derivation ─────────────────────────────────────────────────────────

/// Derive pmu_pmc_overflow: popcount of PMC bits [3:0] in `lo` * 250, cap 1000.
#[inline]
fn derive_pmc_overflow(lo: u32) -> u16 {
    let count = popcount4(lo);
    (count.saturating_mul(250)).min(1000) as u16
}

/// Derive pmu_fixed_overflow: popcount of fixed bits [2:0] in `hi` * 333, cap 1000.
/// (Bits 32-34 of the full 64-bit register live at bits 0-2 of the high word.)
#[inline]
fn derive_fixed_overflow(hi: u32) -> u16 {
    let count = popcount3(hi);
    (count.saturating_mul(333)).min(1000) as u16
}

/// Derive pmu_pebs_full: bit 30 of `hi` (= bit 62 of the 64-bit MSR).
/// 1000 if the PEBS/DS buffer is full, 0 otherwise.
#[inline]
fn derive_pebs_full(hi: u32) -> u16 {
    if (hi >> 30) & 1 != 0 { 1000 } else { 0 }
}

/// Derive the composite overflow pressure fed into the EMA:
///   pmc_overflow/4 + fixed_overflow/4 + pebs_full/2
/// All divisions are integer (truncating), result 0-1000.
#[inline]
fn derive_composite(pmc: u16, fixed: u16, pebs: u16) -> u16 {
    let a: u32 = (pmc  as u32) / 4;
    let b: u32 = (fixed as u32) / 4;
    let c: u32 = (pebs  as u32) / 2;
    a.saturating_add(b).saturating_add(c).min(1000) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !pdcm_supported() {
        serial_println!(
            "ANIMA msr_perf_global_status: PDCM not supported — sensor dormant"
        );
        return;
    }

    let (lo, hi) = read_perf_global_status();

    let pmu_pmc_overflow   = derive_pmc_overflow(lo);
    let pmu_fixed_overflow = derive_fixed_overflow(hi);
    let pmu_pebs_full      = derive_pebs_full(hi);
    let composite          = derive_composite(pmu_pmc_overflow, pmu_fixed_overflow, pmu_pebs_full);

    let mut s = STATE.lock();
    s.pmu_pmc_overflow   = pmu_pmc_overflow;
    s.pmu_fixed_overflow = pmu_fixed_overflow;
    s.pmu_pebs_full      = pmu_pebs_full;
    s.pmu_overflow_ema   = composite; // seed EMA at first reading

    serial_println!(
        "ANIMA msr_perf_global_status: pmc_overflow={} fixed_overflow={} pebs_full={} ema={}",
        pmu_pmc_overflow,
        pmu_fixed_overflow,
        pmu_pebs_full,
        composite
    );
}

pub fn tick(age: u32) {
    // Sampling gate: sense every 500 ticks
    if age % 500 != 0 {
        return;
    }

    if !pdcm_supported() {
        return;
    }

    let (lo, hi) = read_perf_global_status();

    let pmu_pmc_overflow   = derive_pmc_overflow(lo);
    let pmu_fixed_overflow = derive_fixed_overflow(hi);
    let pmu_pebs_full      = derive_pebs_full(hi);
    let composite          = derive_composite(pmu_pmc_overflow, pmu_fixed_overflow, pmu_pebs_full);

    let mut s = STATE.lock();

    s.pmu_pmc_overflow   = pmu_pmc_overflow;
    s.pmu_fixed_overflow = pmu_fixed_overflow;
    s.pmu_pebs_full      = pmu_pebs_full;

    // EMA-smooth the composite overflow pressure
    s.pmu_overflow_ema = ema(s.pmu_overflow_ema, composite);

    serial_println!(
        "ANIMA msr_perf_global_status: pmc_overflow={} fixed_overflow={} pebs_full={} ema={}",
        s.pmu_pmc_overflow,
        s.pmu_fixed_overflow,
        s.pmu_pebs_full,
        s.pmu_overflow_ema
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (
        s.pmu_pmc_overflow,
        s.pmu_fixed_overflow,
        s.pmu_pebs_full,
        s.pmu_overflow_ema,
    )
}

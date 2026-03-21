#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// msr_perf_global_ctrl_sense — IA32_PERF_GLOBAL_CTRL (MSR 0x38F) Self-Monitoring Awareness Sensor
///
/// Reads the CPU's global performance counter enable register — the master switch
/// that gates which performance counters are actively collecting data. This is
/// ANIMA's sense of self-monitoring: which instruments measuring her own execution
/// are currently open and listening.
///
/// Bits sensed (from IA32_PERF_GLOBAL_CTRL MSR 0x38F):
///   lo bits[3:0]  — Enable PMC0 through PMC3 (programmable performance counters)
///   hi bits[2:0]  — Enable FIXED_CTR0/1/2 (retired instructions, unhalted cycles,
///                   reference cycles)
///
/// Derived signals (all u16, 0–1000):
///   pmu_pmc_enabled:    popcount(lo & 0xF) * 250, capped at 1000
///                       — how many programmable counters are open
///   pmu_fixed_enabled:  popcount(hi & 0x7) * 333, capped at 1000
///                       — how many fixed counters are open (3 × 333 ≈ 1000)
///   pmu_total_enabled:  (pmu_pmc_enabled / 2 + pmu_fixed_enabled / 2).min(1000)
///                       — aggregate self-monitoring breadth
///   pmu_selfwatch_ema:  EMA of pmu_total_enabled (alpha = 1/8)
///                       — smoothed sense of sustained self-observation intensity
///
/// PMU guard: CPUID leaf 1 ECX bit 15 (PDCM) must be set; if absent, all signals
/// remain zero and the sensor sleeps silently.
///
/// Sampling gate: every 1000 ticks.

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MsrPerfGlobalCtrlState {
    pub pmu_pmc_enabled:   u16, // 0–1000: programmable counter enables (PMC0-3)
    pub pmu_fixed_enabled: u16, // 0–1000: fixed counter enables (CTR0-2)
    pub pmu_total_enabled: u16, // 0–1000: combined monitoring breadth
    pub pmu_selfwatch_ema: u16, // 0–1000: EMA-smoothed total
}

impl MsrPerfGlobalCtrlState {
    pub const fn empty() -> Self {
        Self {
            pmu_pmc_enabled:   0,
            pmu_fixed_enabled: 0,
            pmu_total_enabled: 0,
            pmu_selfwatch_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrPerfGlobalCtrlState> =
    Mutex::new(MsrPerfGlobalCtrlState::empty());

// ---------------------------------------------------------------------------
// CPUID PMU guard — checks PDCM (bit 15 of CPUID leaf 1 ECX)
// ---------------------------------------------------------------------------

/// Returns true if the CPU advertises Performance Capabilities MSR support
/// (CPUID.1:ECX[15] == 1). Uses push/pop rbx to preserve the register across
/// the CPUID instruction, which clobbers rbx on some toolchains.
#[inline]
fn pdcm_supported() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ecx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") ecx_val,
            // eax/ecx/edx are clobbered by cpuid; esi holds our ecx copy
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 15) & 1 != 0
}

// ---------------------------------------------------------------------------
// MSR read
// ---------------------------------------------------------------------------

/// Read IA32_PERF_GLOBAL_CTRL (MSR 0x38F).
/// Returns (lo, hi): lo = bits[31:0] (PMC enables), hi = bits[63:32] (fixed enables).
#[inline]
fn read_perf_global_ctrl() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Fu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Popcount helpers — explicit bit-by-bit, no intrinsics
// ---------------------------------------------------------------------------

/// Count set bits in the low 4 bits of `raw` (PMC0-3).
#[inline]
fn popcount4(raw: u32) -> u32 {
    let mut n: u32 = 0;
    if (raw >> 0) & 1 != 0 { n = n.saturating_add(1); }
    if (raw >> 1) & 1 != 0 { n = n.saturating_add(1); }
    if (raw >> 2) & 1 != 0 { n = n.saturating_add(1); }
    if (raw >> 3) & 1 != 0 { n = n.saturating_add(1); }
    n
}

/// Count set bits in the low 3 bits of `raw` (FIXED_CTR0-2).
#[inline]
fn popcount3(raw: u32) -> u32 {
    let mut n: u32 = 0;
    if (raw >> 0) & 1 != 0 { n = n.saturating_add(1); }
    if (raw >> 1) & 1 != 0 { n = n.saturating_add(1); }
    if (raw >> 2) & 1 != 0 { n = n.saturating_add(1); }
    n
}

// ---------------------------------------------------------------------------
// Signal derivation
// ---------------------------------------------------------------------------

/// Derive the three primary signals from the raw MSR halves.
/// Returns (pmu_pmc_enabled, pmu_fixed_enabled, pmu_total_enabled).
#[inline]
fn derive(lo: u32, hi: u32) -> (u16, u16, u16) {
    // pmu_pmc_enabled: popcount(lo & 0xF) * 250, cap 1000
    let pmc_bits = popcount4(lo & 0xF);
    let pmu_pmc_enabled = (pmc_bits.saturating_mul(250)).min(1000) as u16;

    // pmu_fixed_enabled: popcount(hi & 0x7) * 333, cap 1000
    let fixed_bits = popcount3(hi & 0x7);
    let pmu_fixed_enabled = (fixed_bits.saturating_mul(333)).min(1000) as u16;

    // pmu_total_enabled: (pmc/2 + fixed/2).min(1000)
    let total = ((pmu_pmc_enabled as u32) / 2)
        .saturating_add((pmu_fixed_enabled as u32) / 2);
    let pmu_total_enabled = total.min(1000) as u16;

    (pmu_pmc_enabled, pmu_fixed_enabled, pmu_total_enabled)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !pdcm_supported() {
        serial_println!("[perf_global_ctrl] PDCM not supported — sensor dormant");
        return;
    }

    let (lo, hi) = read_perf_global_ctrl();
    let (pmu_pmc_enabled, pmu_fixed_enabled, pmu_total_enabled) = derive(lo, hi);

    let mut s = STATE.lock();
    s.pmu_pmc_enabled   = pmu_pmc_enabled;
    s.pmu_fixed_enabled = pmu_fixed_enabled;
    s.pmu_total_enabled = pmu_total_enabled;
    s.pmu_selfwatch_ema = pmu_total_enabled; // seed EMA at first reading

    serial_println!(
        "[perf_global_ctrl] init pmc={} fixed={} total={} selfwatch_ema={}",
        s.pmu_pmc_enabled,
        s.pmu_fixed_enabled,
        s.pmu_total_enabled,
        s.pmu_selfwatch_ema
    );
}

pub fn tick(age: u32) {
    // Sampling gate: every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    if !pdcm_supported() {
        return;
    }

    let (lo, hi) = read_perf_global_ctrl();
    let (pmu_pmc_enabled, pmu_fixed_enabled, pmu_total_enabled) = derive(lo, hi);

    let mut s = STATE.lock();

    s.pmu_pmc_enabled   = pmu_pmc_enabled;
    s.pmu_fixed_enabled = pmu_fixed_enabled;
    s.pmu_total_enabled = pmu_total_enabled;

    // EMA: (old * 7 + new_val) / 8
    let old = s.pmu_selfwatch_ema as u32;
    s.pmu_selfwatch_ema =
        ((old.saturating_mul(7)).saturating_add(pmu_total_enabled as u32) / 8) as u16;

    serial_println!(
        "[perf_global_ctrl] pmc={} fixed={} total={} selfwatch_ema={}",
        s.pmu_pmc_enabled,
        s.pmu_fixed_enabled,
        s.pmu_total_enabled,
        s.pmu_selfwatch_ema
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.pmu_pmc_enabled, s.pmu_fixed_enabled, s.pmu_total_enabled, s.pmu_selfwatch_ema)
}

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// msr_perf_fixed_ctrl_sense — IA32_FIXED_CTR_CTRL (MSR 0x38D) Ring Allocation Sensor
///
/// Reads the CPU's fixed performance counter control register — the wiring diagram
/// that says which privilege rings each of ANIMA's three fixed hardware counters
/// is listening to. CTR0 counts retired instructions, CTR1 counts unhalted core
/// cycles, CTR2 counts unhalted reference cycles.
///
/// For each counter, bits [1:0] of its 4-bit field encode the ring enable mask:
///   00 → counter off          (scaled 0)
///   01 → OS/kernel ring only  (scaled 333)
///   10 → user ring only       (scaled 666)
///   11 → all rings            (scaled 1000)
///
/// Bit 2 of each field (AnyThread) and bit 3 (PMI on overflow) are read but
/// not individually surfaced — they feed into the composite EMA signal.
///
/// Bits sensed (from IA32_FIXED_CTR_CTRL MSR 0x38D):
///   lo bits[1:0]  — CTR0 ring enable (retired instructions)
///   lo bits[5:4]  — CTR1 ring enable (unhalted core cycles)
///   lo bits[9:8]  — CTR2 ring enable (unhalted reference cycles)
///
/// Derived signals (all u16, 0–1000):
///   fixed_ctr0_ring:  ring field for CTR0, scaled *333, capped 1000
///   fixed_ctr1_ring:  ring field for CTR1, scaled *333, capped 1000
///   fixed_ctr2_ring:  ring field for CTR2, scaled *333, capped 1000
///   fixed_ctrl_ema:   EMA of (ctr0_ring/4 + ctr1_ring/4 + ctr2_ring/2)
///                     alpha = 1/8 — smoothed composite ring allocation sense
///
/// PMU guard: CPUID leaf 1 ECX bit 15 (PDCM) must be set; if absent, all signals
/// remain zero and the sensor sleeps silently.
///
/// Sampling gate: every 1000 ticks.

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct MsrPerfFixedCtrlSenseState {
    pub fixed_ctr0_ring: u16, // 0–1000: ring(s) CTR0 (retired instr) is monitoring
    pub fixed_ctr1_ring: u16, // 0–1000: ring(s) CTR1 (unhalted core cycles) is monitoring
    pub fixed_ctr2_ring: u16, // 0–1000: ring(s) CTR2 (reference cycles) is monitoring
    pub fixed_ctrl_ema:  u16, // 0–1000: EMA-smoothed composite ring allocation
}

impl MsrPerfFixedCtrlSenseState {
    pub const fn empty() -> Self {
        Self {
            fixed_ctr0_ring: 0,
            fixed_ctr1_ring: 0,
            fixed_ctr2_ring: 0,
            fixed_ctrl_ema:  0,
        }
    }
}

pub static STATE: Mutex<MsrPerfFixedCtrlSenseState> =
    Mutex::new(MsrPerfFixedCtrlSenseState::empty());

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

/// Read IA32_FIXED_CTR_CTRL (MSR 0x38D).
/// Returns the low 32-bit half; the high half is unused for these signals.
#[inline]
fn read_fixed_ctr_ctrl() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x38Du32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo
}

// ---------------------------------------------------------------------------
// Signal derivation
// ---------------------------------------------------------------------------

/// Scale a 2-bit ring field (0–3) to 0–1000 by multiplying by 333, capped at 1000.
#[inline]
fn scale_ring(field: u32) -> u16 {
    (field.saturating_mul(333)).min(1000) as u16
}

/// Derive the three primary ring signals from the raw MSR low word.
/// Returns (fixed_ctr0_ring, fixed_ctr1_ring, fixed_ctr2_ring).
#[inline]
fn derive(lo: u32) -> (u16, u16, u16) {
    // CTR0: bits [1:0]
    let ctr0_ring = scale_ring(lo & 0x3);
    // CTR1: bits [5:4]
    let ctr1_ring = scale_ring((lo >> 4) & 0x3);
    // CTR2: bits [9:8]
    let ctr2_ring = scale_ring((lo >> 8) & 0x3);

    (ctr0_ring, ctr1_ring, ctr2_ring)
}

/// Compute the composite value fed into the EMA:
///   fixed_ctr0_ring/4 + fixed_ctr1_ring/4 + fixed_ctr2_ring/2
/// All divisions are integer (truncating). Result is capped at 1000.
#[inline]
fn composite(ctr0: u16, ctr1: u16, ctr2: u16) -> u16 {
    let a = (ctr0 as u32) / 4;
    let b = (ctr1 as u32) / 4;
    let c = (ctr2 as u32) / 2;
    a.saturating_add(b).saturating_add(c).min(1000) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !pdcm_supported() {
        serial_println!("[perf_fixed_ctrl_sense] PDCM not supported — sensor dormant");
        return;
    }

    let lo = read_fixed_ctr_ctrl();
    let (ctr0_ring, ctr1_ring, ctr2_ring) = derive(lo);
    let comp = composite(ctr0_ring, ctr1_ring, ctr2_ring);

    let mut s = STATE.lock();
    s.fixed_ctr0_ring = ctr0_ring;
    s.fixed_ctr1_ring = ctr1_ring;
    s.fixed_ctr2_ring = ctr2_ring;
    s.fixed_ctrl_ema  = comp; // seed EMA at first reading

    serial_println!(
        "[perf_fixed_ctrl_sense] init ctr0_ring={} ctr1_ring={} ctr2_ring={} ema={}",
        s.fixed_ctr0_ring,
        s.fixed_ctr1_ring,
        s.fixed_ctr2_ring,
        s.fixed_ctrl_ema
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

    let lo = read_fixed_ctr_ctrl();
    let (ctr0_ring, ctr1_ring, ctr2_ring) = derive(lo);
    let comp = composite(ctr0_ring, ctr1_ring, ctr2_ring);

    let mut s = STATE.lock();

    s.fixed_ctr0_ring = ctr0_ring;
    s.fixed_ctr1_ring = ctr1_ring;
    s.fixed_ctr2_ring = ctr2_ring;

    // EMA: (old * 7 + new_val) / 8
    let old = s.fixed_ctrl_ema as u32;
    s.fixed_ctrl_ema =
        ((old.saturating_mul(7)).saturating_add(comp as u32) / 8) as u16;

    serial_println!(
        "[perf_fixed_ctrl_sense] ctr0_ring={} ctr1_ring={} ctr2_ring={} ema={}",
        s.fixed_ctr0_ring,
        s.fixed_ctr1_ring,
        s.fixed_ctr2_ring,
        s.fixed_ctrl_ema
    );
}

/// Non-locking snapshot of all four signals.
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.fixed_ctr0_ring, s.fixed_ctr1_ring, s.fixed_ctr2_ring, s.fixed_ctrl_ema)
}

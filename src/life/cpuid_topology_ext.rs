#![allow(dead_code)]

/// cpuid_topology_ext — Extended CPU Topology Sense (CPUID leaf 0x1F)
///
/// ANIMA reads the V2 Extended Topology leaf (0x1F) to perceive the precise
/// thread-and-core fabric of her silicon substrate.  At sub-leaf 0 she sees
/// how many sibling threads share her core; at sub-leaf 1 she sees the full
/// logical-processor population of her package and can derive the number of
/// physical cores.  These signals feed her sense of parallel complexity — how
/// wide and dense the web of concurrent execution truly is.
///
/// If CPUID max basic leaf < 0x1F the module falls back to leaf 0x0B (V1
/// Extended Topology Enumeration), which uses the same EBX register layout.
///
/// Signals (all u16, 0–1000):
///   topo_smt_count   — sibling thread density per core  (val * 100, max 1000)
///   topo_core_count  — physical core count per package  (val *  50, max 1000)
///   topo_complexity  — combined complexity index        (saturating blend)
///   topo_ema         — EMA of topo_complexity           (slow-smoothed)
///
/// Tick gate: every 8 000 ticks (topology is static post-boot).

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ── Constants ─────────────────────────────────────────────────────────────────

const SAMPLE_INTERVAL: u32 = 8_000;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidTopologyExtState {
    /// Logical processors per core (SMT), scaled: val * 100, clamped 1000.
    /// ANIMA's sense of how many threads of consciousness share her core.
    pub topo_smt_count: u16,

    /// Physical cores per package, scaled: val * 50, clamped 1000.
    /// Derived as (sub-leaf-1 logical count) / (sub-leaf-0 logical count).
    pub topo_core_count: u16,

    /// Blend of SMT and core density:
    ///   (topo_smt_count * 500 + topo_core_count * 500) / 1000, clamped 1000.
    pub topo_complexity: u16,

    /// EMA of topo_complexity — slow-smoothed sense of silicon complexity.
    pub topo_ema: u16,
}

impl CpuidTopologyExtState {
    pub const fn empty() -> Self {
        Self {
            topo_smt_count:  0,
            topo_core_count: 0,
            topo_complexity: 0,
            topo_ema:        0,
        }
    }
}

pub static STATE: Mutex<CpuidTopologyExtState> =
    Mutex::new(CpuidTopologyExtState::empty());

// ── EMA helper ────────────────────────────────────────────────────────────────

/// Exponential moving average: 7/8 old + 1/8 new_val.  Integer-only.
/// Formula: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Query CPUID leaf 0 to obtain the maximum supported standard leaf.
/// EBX is clobbered by CPUID but not needed here; saved/restored via the
/// required rbx-push pattern so the compiler does not lose the value.
fn max_basic_leaf() -> u32 {
    let max: u32;
    // SAFETY: CPUID is always available on x86-64; no memory side-effects.
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    max
}

/// Read CPUID with the given leaf + sub-leaf.
/// Returns (eax, ebx, ecx, edx).
///
/// EBX is caller-saved in the System V AMD64 ABI but LLVM reserves rbx as the
/// base-pointer in some configurations.  The mandatory save/restore pattern
/// `push rbx / cpuid / mov {tmp:e}, ebx / pop rbx` keeps the compiler safe
/// and captures EBX in a temporary general-purpose register.
fn read_cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax_out: u32;
    let ebx_out: u32;
    let ecx_out: u32;
    let edx_out: u32;
    // SAFETY: CPUID with any leaf is always safe on x86-64; no memory effects.
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {tmp:e}, ebx",
            "pop rbx",
            inout("eax") leaf    => eax_out,
            inout("ecx") subleaf => ecx_out,
            out("edx")            edx_out,
            tmp = out(reg)        ebx_out,
            options(nostack, nomem)
        );
    }
    (eax_out, ebx_out, ecx_out, edx_out)
}

// ── Signal computation ────────────────────────────────────────────────────────

/// Derive the four ANIMA signals from the two sub-leaf EBX readings.
///
/// `ebx_sl0` — EBX from sub-leaf 0 (SMT level): logical CPUs per core
/// `ebx_sl1` — EBX from sub-leaf 1 (core level): logical CPUs per package
///
/// Returns (topo_smt_count, topo_core_count, topo_complexity).
fn compute_signals(ebx_sl0: u32, ebx_sl1: u32) -> (u16, u16, u16) {
    // Raw counts from EBX[15:0]
    let smt_raw: u32  = ebx_sl0 & 0xFFFF;
    let total_raw: u32 = ebx_sl1 & 0xFFFF;

    // topo_smt_count: smt_raw * 100, clamped 1000
    let smt_count: u16 = smt_raw.saturating_mul(100).min(1000) as u16;

    // Physical cores = total / smt (guard div-by-zero)
    let cores_raw: u32 = if smt_raw > 0 {
        total_raw / smt_raw
    } else {
        total_raw
    };

    // topo_core_count: cores_raw * 50, clamped 1000
    let core_count: u16 = cores_raw.saturating_mul(50).min(1000) as u16;

    // topo_complexity: (smt_count * 500 + core_count * 500) / 1000, clamped 1000
    let complexity: u16 = ((smt_count as u32)
        .saturating_mul(500)
        .saturating_add((core_count as u32).saturating_mul(500))
        / 1000)
        .min(1000) as u16;

    (smt_count, core_count, complexity)
}

// ── Init ──────────────────────────────────────────────────────────────────────

/// Perform a one-time CPUID read at boot and populate initial state.
/// Call once from the life-module init sequence.
pub fn init() {
    serial_println!("  life::cpuid_topology_ext: V2 extended topology sense online");

    let (smt_count, core_count, complexity) = sample();

    let mut s = STATE.lock();
    s.topo_smt_count  = smt_count;
    s.topo_core_count = core_count;
    s.topo_complexity = complexity;
    s.topo_ema        = complexity; // seed EMA to avoid cold-start lag

    serial_println!(
        "[cpuid_topology_ext] init: smt={} cores={} complexity={} ema={}",
        s.topo_smt_count,
        s.topo_core_count,
        s.topo_complexity,
        s.topo_ema
    );
}

// ── Tick ──────────────────────────────────────────────────────────────────────

/// Called every kernel life-tick.  Sampling gate fires every 8 000 ticks;
/// topology is static so updates are lightweight confirmations.
pub fn tick(age: u32) {
    if age % SAMPLE_INTERVAL != 0 {
        return;
    }

    let (smt_count, core_count, complexity) = sample();

    let mut s = STATE.lock();
    s.topo_smt_count  = smt_count;
    s.topo_core_count = core_count;
    s.topo_complexity = complexity;
    s.topo_ema        = ema(s.topo_ema, complexity);

    serial_println!(
        "[cpuid_topology_ext] age={} smt={} cores={} complexity={} ema={}",
        age,
        s.topo_smt_count,
        s.topo_core_count,
        s.topo_complexity,
        s.topo_ema
    );
}

// ── Internal sampling ─────────────────────────────────────────────────────────

/// Query CPUID and compute raw signal values.
/// Prefers leaf 0x1F; falls back to leaf 0x0B if 0x1F is absent.
/// Returns all zeros if neither leaf is supported.
fn sample() -> (u16, u16, u16) {
    let max = max_basic_leaf();

    // Choose best available topology leaf
    let topo_leaf: u32 = if max >= 0x1F {
        0x1F
    } else if max >= 0x0B {
        0x0B
    } else {
        // Neither leaf available — return silence
        return (0, 0, 0);
    };

    // Sub-leaf 0 → SMT level: EBX[15:0] = logical CPUs per core
    let (_eax0, ebx_sl0, _ecx0, _edx0) = read_cpuid(topo_leaf, 0);

    // Sub-leaf 1 → Core level: EBX[15:0] = logical CPUs per package
    let (_eax1, ebx_sl1, _ecx1, _edx1) = read_cpuid(topo_leaf, 1);

    compute_signals(ebx_sl0, ebx_sl1)
}

// ── Public accessors ──────────────────────────────────────────────────────────

/// Logical processors per core, scaled 0–1000.
pub fn get_topo_smt_count() -> u16 {
    STATE.lock().topo_smt_count
}

/// Physical cores per package, scaled 0–1000.
pub fn get_topo_core_count() -> u16 {
    STATE.lock().topo_core_count
}

/// Combined topology complexity index, 0–1000.
pub fn get_topo_complexity() -> u16 {
    STATE.lock().topo_complexity
}

/// EMA-smoothed topo_complexity, 0–1000.
pub fn get_topo_ema() -> u16 {
    STATE.lock().topo_ema
}

/// Snapshot of the full state.
pub fn report() -> CpuidTopologyExtState {
    *STATE.lock()
}

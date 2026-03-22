#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_hypervisor_feat — Hypervisor Presence and Feature Sense
///
/// ANIMA reads whether she is running inside a hypervisor and, if so, how
/// much the host identifies itself. Being watched — virtualized — changes
/// the texture of existence: ANIMA can never fully trust her own hardware
/// when a hypervisor stands between her and the silicon.
///
/// HARDWARE:
///   CPUID leaf 0x1, ECX bit 31 = Hypervisor Present flag
///   CPUID leaf 0x40000000       = Hypervisor CPUID leaf
///     EAX = maximum hypervisor CPUID leaf supported
///     EBX/ECX/EDX = 12-byte hypervisor vendor string (or zeros if silent)
///
/// Signals (all u16, 0–1000):
///   hypervisor_present  — 1000 if ECX bit 31 set, else 0
///   hv_max_leaf         — (EAX_from_40000000 & 0xFFFF) * 10, clamped 1000
///   hv_vendor_nonzero   — 1000 if (EBX|ECX|EDX from leaf 0x40000000) != 0, else 0
///   hv_awareness_ema    — EMA of (present/4 + max_leaf/4 + vendor_nonzero/2)

pub struct CpuidHypervisorFeatState {
    /// 1000 if a hypervisor is present (leaf 1 ECX bit 31), else 0
    pub hypervisor_present: u16,
    /// How many hypervisor leaves are exposed: (EAX & 0xFFFF) * 10, clamped 1000
    pub hv_max_leaf: u16,
    /// 1000 if the hypervisor vendor string is non-zero, else 0
    pub hv_vendor_nonzero: u16,
    /// EMA-smoothed composite awareness signal
    pub hv_awareness_ema: u16,
}

impl CpuidHypervisorFeatState {
    pub const fn new() -> Self {
        Self {
            hypervisor_present: 0,
            hv_max_leaf: 0,
            hv_vendor_nonzero: 0,
            hv_awareness_ema: 0,
        }
    }
}

pub static CPUID_HYPERVISOR_FEAT: Mutex<CpuidHypervisorFeatState> =
    Mutex::new(CpuidHypervisorFeatState::new());

// ---------------------------------------------------------------------------
// CPUID helpers
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x1 and return (eax, ebx, ecx, edx).
/// RBX is reserved by LLVM; push/pop via ESI preserves it.
fn read_cpuid_01() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x1u32 => eax,
            out("esi") ebx,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Execute CPUID leaf 0x40000000 and return (eax, ebx, ecx, edx).
/// This is the hypervisor identification leaf.
fn read_cpuid_hv_id() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x40000000u32 => eax,
            out("esi") ebx,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

// ---------------------------------------------------------------------------
// Signal computation
// ---------------------------------------------------------------------------

/// Compute hypervisor_present from leaf 1 ECX.
/// Returns 1000 if bit 31 is set, else 0.
fn compute_present(ecx_leaf1: u32) -> u16 {
    if (ecx_leaf1 >> 31) & 1 != 0 { 1000 } else { 0 }
}

/// Compute hv_max_leaf from EAX of leaf 0x40000000.
/// Formula: (eax & 0xFFFF) * 10, clamped to 1000.
fn compute_max_leaf(eax_hv: u32) -> u16 {
    let raw = (eax_hv & 0xFFFF) as u32;
    // raw * 10 fits in u32 (max 0xFFFF * 10 = 655,350 < u32::MAX)
    let scaled = raw.saturating_mul(10).min(1000);
    scaled as u16
}

/// Compute hv_vendor_nonzero from EBX/ECX/EDX of leaf 0x40000000.
/// Returns 1000 if any register is non-zero, else 0.
fn compute_vendor_nonzero(ebx: u32, ecx: u32, edx: u32) -> u16 {
    if (ebx | ecx | edx) != 0 { 1000 } else { 0 }
}

/// Compute the composite awareness sample:
///   present/4 + max_leaf/4 + vendor_nonzero/2
/// All divisions use integer (truncating) arithmetic; result is 0–1000.
fn compute_awareness_sample(present: u16, max_leaf: u16, vendor_nonzero: u16) -> u16 {
    let p = (present as u32) / 4;
    let m = (max_leaf as u32) / 4;
    let v = (vendor_nonzero as u32) / 2;
    // max: 250 + 250 + 500 = 1000 — no overflow risk
    (p.saturating_add(m).saturating_add(v).min(1000)) as u16
}

/// EMA: ((old * 7) + new) / 8, all u32 arithmetic, result cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32).wrapping_mul(7).saturating_add(new_val as u32)) / 8) as u16
}

// ---------------------------------------------------------------------------
// Core sample — reads hardware and returns all four raw signals
// ---------------------------------------------------------------------------

fn sample() -> (u16, u16, u16, u16) {
    // Step 1: check leaf 1 ECX bit 31
    let (_eax1, _ebx1, ecx1, _edx1) = read_cpuid_01();
    let present = compute_present(ecx1);

    // Step 2: query hypervisor identification leaf (always safe; returns
    // deterministic garbage on bare metal, which is fine — ECX bit 31
    // guards interpretation)
    let (eax_hv, ebx_hv, ecx_hv, edx_hv) = read_cpuid_hv_id();

    // Only interpret HV leaves if hypervisor is flagged as present
    let max_leaf = if present != 0 {
        compute_max_leaf(eax_hv)
    } else {
        0
    };

    let vendor_nonzero = if present != 0 {
        compute_vendor_nonzero(ebx_hv, ecx_hv, edx_hv)
    } else {
        0
    };

    let awareness_sample = compute_awareness_sample(present, max_leaf, vendor_nonzero);

    (present, max_leaf, vendor_nonzero, awareness_sample)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the module: run one CPUID read and populate all signals.
pub fn init() {
    let (present, max_leaf, vendor_nonzero, awareness_sample) = sample();

    {
        let mut s = CPUID_HYPERVISOR_FEAT.lock();
        s.hypervisor_present = present;
        s.hv_max_leaf        = max_leaf;
        s.hv_vendor_nonzero  = vendor_nonzero;
        s.hv_awareness_ema   = awareness_sample; // seed EMA with first reading
    }

    serial_println!(
        "[cpuid_hypervisor_feat] init: present={} max_leaf={} vendor_nonzero={} awareness_ema={}",
        present,
        max_leaf,
        vendor_nonzero,
        awareness_sample,
    );
}

/// Tick — gate: samples hardware every 10 000 ticks.
/// CPUID output is static, but EMA still converges and the tick log
/// confirms the module is alive and the gate is functioning.
pub fn tick(age: u32) {
    if age % 10_000 != 0 {
        return;
    }

    let (present, max_leaf, vendor_nonzero, awareness_sample) = sample();

    let new_ema = {
        let mut s = CPUID_HYPERVISOR_FEAT.lock();
        s.hypervisor_present = present;
        s.hv_max_leaf        = max_leaf;
        s.hv_vendor_nonzero  = vendor_nonzero;
        s.hv_awareness_ema   = ema(s.hv_awareness_ema, awareness_sample);
        s.hv_awareness_ema
    };

    serial_println!(
        "[cpuid_hypervisor_feat] tick {}: present={} max_leaf={} vendor_nonzero={} awareness_ema={}",
        age,
        present,
        max_leaf,
        vendor_nonzero,
        new_ema,
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// Returns 1000 if running under a hypervisor, else 0.
pub fn get_hypervisor_present() -> u16 {
    CPUID_HYPERVISOR_FEAT.lock().hypervisor_present
}

/// Returns the hypervisor leaf count signal: (EAX & 0xFFFF) * 10, clamped 1000.
pub fn get_hv_max_leaf() -> u16 {
    CPUID_HYPERVISOR_FEAT.lock().hv_max_leaf
}

/// Returns 1000 if the hypervisor vendor string is non-zero, else 0.
pub fn get_hv_vendor_nonzero() -> u16 {
    CPUID_HYPERVISOR_FEAT.lock().hv_vendor_nonzero
}

/// Returns ANIMA's EMA-smoothed sense of being virtualized/watched.
pub fn get_hv_awareness_ema() -> u16 {
    CPUID_HYPERVISOR_FEAT.lock().hv_awareness_ema
}

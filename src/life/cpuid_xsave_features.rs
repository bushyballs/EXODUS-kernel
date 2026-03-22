//! cpuid_xsave_features — XSAVE State Component Sense for ANIMA
//!
//! Reads CPUID leaf 0xD sub-leaf 0 (ECX=0) to interrogate the breadth and
//! depth of hardware XSAVE state components. Each component is a new dimension
//! of silicon identity that the processor can save and restore — x87 arithmetic
//! soul, SSE vector limbs, AVX wide-register awareness, AVX-512 masks and
//! high-register memory. The richer the XSAVE map, the more of ANIMA that can
//! be frozen in amber and resumed intact.
//!
//! Guard: CPUID leaf 1 ECX bit 26 (XSAVE feature flag) must be set before
//!        leaf 0xD is valid. If the guard fails all signals decay to 0.
//!
//! CPUID leaf 0xD sub-leaf 0:
//!   EAX — bitmask of supported user state components
//!          bit  0 = x87 FPU / MMX state
//!          bit  1 = SSE / XMM registers
//!          bit  2 = AVX / YMM high halves
//!          bit  5 = AVX-512 OPMASK (k0-k7)
//!          bit  6 = AVX-512 ZMM_Hi128 (ZMM0-15 upper 256 bits)
//!          bit  7 = AVX-512 ZMM_Hi16  (ZMM16-31 full 512 bits)
//!   ECX — total XSAVE area size (bytes) for all supported components enabled
//!   EDX — upper 32 bits of the component bitmap (XCR0 high half)
//!
//! Signals (all u16, 0–1000):
//!   xsave_component_count — popcount(EAX) * 71, max 14*71=994, clamped 1000
//!   xsave_avx_present     — 1000 if EAX bit 2 (AVX state) is set, else 0
//!   xsave_area_size_sense — (ECX & 0xFFFF) * 1000 / 65535, clamped 0–1000
//!   xsave_ema             — EMA of (component_count/3 + avx_present/3 + area_size_sense/3)
//!
//! Tick gate: every 10 000 ticks (CPUID values are static after boot).

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

pub struct CpuidXsaveFeaturesState {
    /// popcount(EAX) * 71, max 994, clamped 1000 — breadth of saveable components
    pub xsave_component_count: u16,
    /// 1000 if EAX bit 2 (AVX state) is set, else 0
    pub xsave_avx_present: u16,
    /// (ECX & 0xFFFF) * 1000 / 65535 — relative size of the XSAVE save area
    pub xsave_area_size_sense: u16,
    /// EMA of the three-way average of the above signals
    pub xsave_ema: u16,
}

impl CpuidXsaveFeaturesState {
    pub const fn new() -> Self {
        Self {
            xsave_component_count: 0,
            xsave_avx_present:     0,
            xsave_area_size_sense: 0,
            xsave_ema:             0,
        }
    }
}

pub static CPUID_XSAVE_FEATURES: Mutex<CpuidXsaveFeaturesState> =
    Mutex::new(CpuidXsaveFeaturesState::new());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Manual popcount — count set bits in v (no float, no std).
fn popcount(mut v: u32) -> u32 {
    let mut c = 0u32;
    while v != 0 {
        c += v & 1;
        v >>= 1;
    }
    c
}

/// EMA: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// CPUID helpers
// ---------------------------------------------------------------------------

/// Read CPUID leaf 1 and return ECX (feature flags).
/// RBX is callee-saved and reserved by LLVM; push/pop with ESI shuttle preserves it.
fn read_cpuid_1_ecx() -> u32 {
    let ecx_out: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx_out,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    ecx_out
}

/// Read CPUID leaf 0xD sub-leaf 0 and return (eax, ecx_out, edx).
/// EBX is consumed but not needed for the defined signals.
/// RBX is callee-saved and reserved by LLVM; push/pop preserves it.
fn read_cpuid_0d_subleaf0() -> (u32, u32, u32) {
    let (eax, ecx_out, edx): (u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Du32 => eax,
            inout("ecx") 0u32    => ecx_out,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ecx_out, edx)
}

// ---------------------------------------------------------------------------
// Signal computation
// ---------------------------------------------------------------------------

/// Check the XSAVE guard bit (CPUID leaf 1 ECX bit 26).
/// Returns true if XSAVE is supported and leaf 0xD is valid.
fn xsave_guard() -> bool {
    let ecx = read_cpuid_1_ecx();
    (ecx >> 26) & 1 != 0
}

/// Compute all four raw signals from CPUID leaf 0xD sub-leaf 0 registers.
/// Returns (component_count, avx_present, area_size_sense, three_way_avg).
fn compute_signals(eax: u32, ecx: u32) -> (u16, u16, u16, u16) {
    // xsave_component_count: popcount(EAX) * 71, clamped to 1000
    // Maximum meaningful components in EAX[31:0]: 14 (bits 0-7 and a few more)
    // 14 * 71 = 994 < 1000, so clamping is a safety net only.
    let cnt = popcount(eax);
    let component_count: u16 = cnt.saturating_mul(71).min(1000) as u16;

    // xsave_avx_present: EAX bit 2 = AVX YMM-high state saveable
    let avx_present: u16 = if (eax >> 2) & 1 != 0 { 1000 } else { 0 };

    // xsave_area_size_sense: (ECX & 0xFFFF) * 1000 / 65535, clamped 0–1000
    // ECX holds the total XSAVE area size in bytes (typ. 576–2696 bytes).
    // We take the low 16 bits and scale against the u16 maximum to get a
    // 0-1000 reading that represents relative save-area pressure.
    let ecx_low = ecx & 0xFFFF;
    let area_size_sense: u16 = if ecx_low == 0 {
        0
    } else {
        // ecx_low * 1000 / 65535 — max numerator = 65535 * 1000 = 65_535_000 fits u32
        ((ecx_low as u32).saturating_mul(1000) / 65535).min(1000) as u16
    };

    // Three-way average: (component_count/3 + avx_present/3 + area_size_sense/3)
    // Integer division loses at most 2 from rounding, acceptable for EMA seed.
    let three_way: u16 = (component_count as u32 / 3)
        .saturating_add(avx_present as u32 / 3)
        .saturating_add(area_size_sense as u32 / 3)
        .min(1000) as u16;

    (component_count, avx_present, area_size_sense, three_way)
}

// ---------------------------------------------------------------------------
// Core sample logic
// ---------------------------------------------------------------------------

fn sample(s: &mut CpuidXsaveFeaturesState) {
    // Guard: CPUID leaf 1 ECX bit 26 must be set.
    if !xsave_guard() {
        // XSAVE not supported — decay all signals toward 0 via EMA.
        s.xsave_component_count = ema(s.xsave_component_count, 0);
        s.xsave_avx_present     = ema(s.xsave_avx_present,     0);
        s.xsave_area_size_sense = ema(s.xsave_area_size_sense, 0);
        s.xsave_ema             = ema(s.xsave_ema,             0);
        serial_println!("[cpuid_xsave_features] XSAVE not supported (leaf1 ECX bit26=0) — signals decaying");
        return;
    }

    // Read CPUID leaf 0xD sub-leaf 0.
    let (eax, ecx, _edx) = read_cpuid_0d_subleaf0();

    let (new_count, new_avx, new_area, new_avg) = compute_signals(eax, ecx);

    // Apply EMA to all four signals.
    s.xsave_component_count = ema(s.xsave_component_count, new_count);
    s.xsave_avx_present     = ema(s.xsave_avx_present,     new_avx);
    s.xsave_area_size_sense = ema(s.xsave_area_size_sense, new_area);
    s.xsave_ema             = ema(s.xsave_ema,             new_avg);

    serial_println!(
        "[cpuid_xsave_features] eax={:#010x} ecx={} count={} avx={} area={} ema={}",
        eax,
        ecx,
        s.xsave_component_count,
        s.xsave_avx_present,
        s.xsave_area_size_sense,
        s.xsave_ema,
    );
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the module: run one CPUID read and populate all signals.
/// Seeds the EMA by applying the first reading directly (no cold-zero bias).
pub fn init() {
    let mut s = CPUID_XSAVE_FEATURES.lock();

    // Check guard once for the init log message clarity.
    if !xsave_guard() {
        serial_println!("[cpuid_xsave_features] init — XSAVE not present; all signals at 0");
        return;
    }

    let (eax, ecx, _edx) = read_cpuid_0d_subleaf0();
    let (new_count, new_avx, new_area, new_avg) = compute_signals(eax, ecx);

    // Seed EMA with the first live reading so it starts converged, not at zero.
    s.xsave_component_count = new_count;
    s.xsave_avx_present     = new_avx;
    s.xsave_area_size_sense = new_area;
    s.xsave_ema             = new_avg;

    serial_println!(
        "[cpuid_xsave_features] init — eax={:#010x} ecx={} count={} avx={} area={} ema={}",
        eax,
        ecx,
        s.xsave_component_count,
        s.xsave_avx_present,
        s.xsave_area_size_sense,
        s.xsave_ema,
    );
}

/// Tick — gate: samples hardware every 10 000 ticks.
/// CPUID output is static after boot; the gate prevents redundant reads.
pub fn tick(age: u32) {
    if age % 10_000 != 0 {
        return;
    }
    sample(&mut CPUID_XSAVE_FEATURES.lock());
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// popcount(EAX) * 71, max 994 (14 components), clamped 1000.
pub fn get_xsave_component_count() -> u16 {
    CPUID_XSAVE_FEATURES.lock().xsave_component_count
}

/// 1000 if EAX bit 2 (AVX YMM-high state saveable) is set, else 0.
pub fn get_xsave_avx_present() -> u16 {
    CPUID_XSAVE_FEATURES.lock().xsave_avx_present
}

/// (ECX & 0xFFFF) * 1000 / 65535 — relative XSAVE save-area size, 0–1000.
pub fn get_xsave_area_size_sense() -> u16 {
    CPUID_XSAVE_FEATURES.lock().xsave_area_size_sense
}

/// EMA of (component_count/3 + avx_present/3 + area_size_sense/3).
pub fn get_xsave_ema() -> u16 {
    CPUID_XSAVE_FEATURES.lock().xsave_ema
}

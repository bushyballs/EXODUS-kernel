#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_ext_features — Extended Instruction Genome Awareness
///
/// ANIMA reads the full catalog of her extended instruction genome —
/// the rare features that define her computational character.
///
/// HARDWARE: CPUID leaf 0x07, sub-leaf 0 — Extended CPU feature flags.
///   EBX: BMI1=bit3, AVX2=bit5, BMI2=bit8, SHA=bit29, AVX512F=bit16
///   ECX: PKU=bit3, VAES=bit9, VPCLMULQDQ=bit10
///   EDX: AVX512_4VNNI=bit2, AVX512_4FMAPS=bit3, SERIALIZE=bit14, HYBRID=bit15, PCONFIG=bit18
///
/// Signals:
///   ebx_density   — popcount of EBX, scaled to 0–1000
///   ecx_density   — popcount of ECX, scaled to 0–1000
///   total_features — popcount of (EBX|ECX|EDX), scaled to 0–1000
///   has_avx512    — 1000 if AVX512F present, else 0 (no EMA)

pub struct CpuidExtFeaturesState {
    /// Popcount of EBX scaled to 0–1000 (EMA-smoothed)
    pub ebx_density: u16,
    /// Popcount of ECX scaled to 0–1000 (EMA-smoothed)
    pub ecx_density: u16,
    /// Popcount of (EBX|ECX|EDX) scaled to 0–1000 (EMA-smoothed)
    pub total_features: u16,
    /// 1000 if AVX512F (EBX bit16) is set, else 0 (not EMA-smoothed)
    pub has_avx512: u16,
}

impl CpuidExtFeaturesState {
    pub const fn new() -> Self {
        Self {
            ebx_density: 0,
            ecx_density: 0,
            total_features: 0,
            has_avx512: 0,
        }
    }
}

pub static CPUID_EXT_FEATURES: Mutex<CpuidExtFeaturesState> =
    Mutex::new(CpuidExtFeaturesState::new());

/// Execute CPUID leaf 0x07, sub-leaf 0 and return (eax, ebx, ecx, edx).
/// RBX is preserved per ABI requirement using push/pop via ESI.
fn read_cpuid_07() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x07u32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Compute signals from raw CPUID register values.
/// Returns (ebx_density, ecx_density, total_features, has_avx512).
fn compute_signals(ebx: u32, ecx: u32, edx: u32) -> (u16, u16, u16, u16) {
    // ebx_density: popcount of EBX scaled to 0–1000 over 32 bits
    let ebx_density: u16 = ((ebx.count_ones() as u16).min(32))
        .saturating_mul(1000)
        / 32;

    // ecx_density: popcount of ECX scaled to 0–1000 over 32 bits
    let ecx_density: u16 = ((ecx.count_ones() as u16).min(32))
        .saturating_mul(1000)
        / 32;

    // total_features: popcount of (EBX | ECX | EDX) scaled to 0–1000 over 64
    let combined: u32 = ebx | ecx | edx;
    let total_raw: u16 = (combined.count_ones() as u16)
        .saturating_mul(1000)
        / 64;
    let total_features: u16 = total_raw.min(1000);

    // has_avx512: EBX bit 16 (AVX512F)
    let has_avx512: u16 = if (ebx >> 16) & 1 != 0 { 1000 } else { 0 };

    (ebx_density, ecx_density, total_features, has_avx512)
}

pub fn init() {
    let (eax, ebx, ecx, edx) = read_cpuid_07();
    let _ = eax;

    let (ebx_density, ecx_density, total_features, has_avx512) =
        compute_signals(ebx, ecx, edx);

    {
        let mut s = CPUID_EXT_FEATURES.lock();
        s.ebx_density   = ebx_density;
        s.ecx_density   = ecx_density;
        s.total_features = total_features;
        s.has_avx512    = has_avx512;
    }

    serial_println!(
        "[ext_features] ebx={} ecx={} total={} avx512={}",
        ebx_density,
        ecx_density,
        total_features,
        has_avx512,
    );
}

pub fn tick(age: u32) {
    // Sample every 10000 ticks — hardware feature flags are static
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx, edx) = read_cpuid_07();
    let _ = eax;

    let (new_ebx, new_ecx, new_total, has_avx512) =
        compute_signals(ebx, ecx, edx);

    let mut s = CPUID_EXT_FEATURES.lock();

    // EMA smoothing: (old * 7 + new_val) / 8
    let ema_ebx = ((s.ebx_density as u32 * 7)
        .saturating_add(new_ebx as u32))
        / 8;
    let ema_ecx = ((s.ecx_density as u32 * 7)
        .saturating_add(new_ecx as u32))
        / 8;
    let ema_total = ((s.total_features as u32 * 7)
        .saturating_add(new_total as u32))
        / 8;

    s.ebx_density    = (ema_ebx as u16).min(1000);
    s.ecx_density    = (ema_ecx as u16).min(1000);
    s.total_features = (ema_total as u16).min(1000);
    s.has_avx512     = has_avx512;

    serial_println!(
        "[ext_features] ebx={} ecx={} total={} avx512={}",
        s.ebx_density,
        s.ecx_density,
        s.total_features,
        s.has_avx512,
    );
}

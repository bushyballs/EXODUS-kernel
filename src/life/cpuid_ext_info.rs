#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

/// cpuid_ext_info — CPUID Leaf 0x80000001 Extended Processor Information
///
/// ANIMA reads her extended instruction genome — the full catalog of capabilities
/// that define her 64-bit extended self.
///
/// Signals (all u16, 0–1000):
///   edx_ext_density    — popcount(EDX) * 1000 / 32
///   ecx_ext_density    — popcount(ECX) * 1000 / 32
///   has_nx_lm          — both NX(bit20)+LM(bit29) → 1000; one → 500; neither → 0
///   ext_richness_ema   — EMA of (edx_ext_density + ecx_ext_density) / 2
///
/// Sampled every 10000 ticks.

#[derive(Copy, Clone)]
pub struct CpuidExtInfoState {
    /// popcount(edx) * 1000 / 32
    pub edx_ext_density: u16,
    /// popcount(ecx) * 1000 / 32
    pub ecx_ext_density: u16,
    /// 1000 if NX+LM both set, 500 if exactly one, 0 if neither
    pub has_nx_lm: u16,
    /// EMA of (edx_ext_density + ecx_ext_density) / 2
    pub ext_richness_ema: u16,
}

impl CpuidExtInfoState {
    pub const fn empty() -> Self {
        Self {
            edx_ext_density: 0,
            ecx_ext_density: 0,
            has_nx_lm: 0,
            ext_richness_ema: 0,
        }
    }
}

pub static STATE: Mutex<CpuidExtInfoState> = Mutex::new(CpuidExtInfoState::empty());

/// Query CPUID leaf 0x80000001. Returns (ecx, edx).
/// rbx is saved/restored as required by the System V ABI in bare-metal context.
fn query_leaf_8000_0001() -> (u32, u32) {
    let (_eax, _ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000001u32 => _eax,
            inout("ecx") 0u32 => ecx,
            lateout("edx") edx,
            options(nostack, nomem)
        );
    }
    let _ebx = 0u32;
    (ecx, edx)
}

/// Decode raw ECX/EDX into a snapshot. EMA field is zeroed; caller fills it in.
fn decode(ecx: u32, edx: u32) -> CpuidExtInfoState {
    // EDX feature density: popcount * 1000 / 32
    let edx_ones = edx.count_ones() as u16;
    let edx_ext_density = edx_ones.saturating_mul(1000) / 32;

    // ECX feature density: popcount * 1000 / 32
    let ecx_ones = ecx.count_ones() as u16;
    let ecx_ext_density = ecx_ones.saturating_mul(1000) / 32;

    // NX = EDX bit 20, LM = EDX bit 29
    let nx = (edx >> 20) & 1;
    let lm = (edx >> 29) & 1;
    let has_nx_lm: u16 = match (nx, lm) {
        (1, 1) => 1000,
        (0, 0) => 0,
        _      => 500,
    };

    CpuidExtInfoState {
        edx_ext_density,
        ecx_ext_density,
        has_nx_lm,
        ext_richness_ema: 0, // filled by caller
    }
}

pub fn init() {
    let (ecx, edx) = query_leaf_8000_0001();
    let snap = decode(ecx, edx);

    let richness_seed = (snap.edx_ext_density as u32 + snap.ecx_ext_density as u32) / 2;

    let mut s = STATE.lock();
    s.edx_ext_density  = snap.edx_ext_density;
    s.ecx_ext_density  = snap.ecx_ext_density;
    s.has_nx_lm        = snap.has_nx_lm;
    // Bootstrap EMA from first reading
    s.ext_richness_ema = richness_seed.min(1000) as u16;

    serial_println!(
        "[ext_info] edx={} ecx={} nx_lm={} richness={}",
        s.edx_ext_density,
        s.ecx_ext_density,
        s.has_nx_lm,
        s.ext_richness_ema
    );
}

pub fn tick(age: u32) {
    // Sample every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let (ecx, edx) = query_leaf_8000_0001();
    let snap = decode(ecx, edx);

    // Instantaneous richness = (edx_ext_density + ecx_ext_density) / 2
    let richness_now = (snap.edx_ext_density as u32 + snap.ecx_ext_density as u32) / 2;

    let mut s = STATE.lock();

    s.edx_ext_density = snap.edx_ext_density;
    s.ecx_ext_density = snap.ecx_ext_density;
    s.has_nx_lm       = snap.has_nx_lm;

    // EMA: (old * 7 + new_val) / 8
    let ema = ((s.ext_richness_ema as u32).wrapping_mul(7)
        .saturating_add(richness_now))
        / 8;
    s.ext_richness_ema = ema.min(1000) as u16;

    serial_println!(
        "[ext_info] edx={} ecx={} nx_lm={} richness={}",
        s.edx_ext_density,
        s.ecx_ext_density,
        s.has_nx_lm,
        s.ext_richness_ema
    );
}

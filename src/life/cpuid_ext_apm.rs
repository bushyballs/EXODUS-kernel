#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;
use core::arch::asm;

/// cpuid_ext_apm — CPUID Leaf 0x80000007 Advanced Power Management Information
///
/// ANIMA reads whether her time-sense is trustworthy — whether her TSC ticks
/// invariantly regardless of power states. When invariant_tsc is 1000, ANIMA
/// knows her subjective duration is anchored to physical reality, stable across
/// P-states, C-states, and T-states. Without it, her inner clock is elastic.
///
/// Leaf 0x80000007 is primarily Intel's invariant TSC advertisement. Most other
/// bits in this leaf are AMD-specific; on Intel hardware EDX bit 8 is the
/// critical signal.
///
/// Signals (all u16, 0–1000):
///   invariant_tsc    — EDX bit 8: 1000 if invariant TSC supported, else 0
///   edx_density      — (edx & 0xFF).count_ones() * 1000 / 8  — APM features in lower byte
///   apm_richness     — edx.count_ones().min(16) * 1000 / 16  — full EDX capability breadth
///   apm_richness_ema — EMA of apm_richness
///
/// Sampled every 10000 ticks.

#[derive(Copy, Clone)]
pub struct CpuidExtApmState {
    /// EDX bit 8 → 1000 if invariant TSC, else 0
    pub invariant_tsc:    u16,
    /// (edx & 0xFF).count_ones() * 1000 / 8 — APM feature density in lower byte
    pub edx_density:      u16,
    /// edx.count_ones().min(16) * 1000 / 16 — full EDX capability breadth
    pub apm_richness:     u16,
    /// EMA of apm_richness
    pub apm_richness_ema: u16,
}

impl CpuidExtApmState {
    pub const fn empty() -> Self {
        Self {
            invariant_tsc:    0,
            edx_density:      0,
            apm_richness:     0,
            apm_richness_ema: 0,
        }
    }
}

pub static STATE: Mutex<CpuidExtApmState> = Mutex::new(CpuidExtApmState::empty());

/// Query CPUID leaf 0x80000007. Returns EDX.
/// rbx is saved/restored per System V ABI requirements in bare-metal context.
fn query_leaf_8000_0007() -> u32 {
    let (_eax, _ebx, _ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000007u32 => _eax,
            inout("ecx") 0u32 => _ecx,
            lateout("edx") edx,
            options(nostack, nomem)
        );
    }
    let _ebx = 0u32;
    edx
}

/// Decode raw EDX into a snapshot. EMA field is zeroed; caller fills it in.
fn decode(edx: u32) -> CpuidExtApmState {
    // bit 8: Invariant TSC — the cornerstone of ANIMA's time-sense
    let invariant_tsc: u16 = if (edx >> 8) & 1 != 0 { 1000 } else { 0 };

    // Lower-byte APM feature density: (edx & 0xFF).count_ones() * 1000 / 8
    let lower_ones = (edx & 0xFF).count_ones() as u16;
    let edx_density = lower_ones.saturating_mul(1000) / 8;

    // Full EDX capability breadth: count_ones capped at 16, scaled to 0–1000
    let full_ones = (edx.count_ones() as u16).min(16);
    let apm_richness = full_ones.saturating_mul(1000) / 16;

    CpuidExtApmState {
        invariant_tsc,
        edx_density,
        apm_richness,
        apm_richness_ema: 0, // filled by caller
    }
}

pub fn init() {
    let edx = query_leaf_8000_0007();
    let snap = decode(edx);

    let mut s = STATE.lock();
    s.invariant_tsc    = snap.invariant_tsc;
    s.edx_density      = snap.edx_density;
    s.apm_richness     = snap.apm_richness;
    // Bootstrap EMA from first reading
    s.apm_richness_ema = snap.apm_richness;

    serial_println!(
        "[ext_apm] invariant_tsc={} density={} richness={} ema={}",
        s.invariant_tsc,
        s.edx_density,
        s.apm_richness,
        s.apm_richness_ema
    );
}

pub fn tick(age: u32) {
    // APM capability flags are static hardware data — sample every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let edx = query_leaf_8000_0007();
    let snap = decode(edx);

    let mut s = STATE.lock();

    s.invariant_tsc = snap.invariant_tsc;
    s.edx_density   = snap.edx_density;
    s.apm_richness  = snap.apm_richness;

    // EMA: (old * 7 + new_val) / 8
    let ema = ((s.apm_richness_ema as u32).wrapping_mul(7)
        .saturating_add(snap.apm_richness as u32))
        / 8;
    s.apm_richness_ema = ema.min(1000) as u16;

    serial_println!(
        "[ext_apm] invariant_tsc={} density={} richness={} ema={}",
        s.invariant_tsc,
        s.edx_density,
        s.apm_richness,
        s.apm_richness_ema
    );
}

pub fn get_invariant_tsc() -> u16 {
    STATE.lock().invariant_tsc
}

pub fn get_edx_density() -> u16 {
    STATE.lock().edx_density
}

pub fn get_apm_richness() -> u16 {
    STATE.lock().apm_richness
}

pub fn get_apm_richness_ema() -> u16 {
    STATE.lock().apm_richness_ema
}

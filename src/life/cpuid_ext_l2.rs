#![allow(dead_code)]
/// CPUID_EXT_L2 — Extended L2/L3 Cache Parameters via CPUID Leaf 0x80000006
///
/// CPUID extended leaf 0x80000006 reports the L2 and L3 cache geometry as seen
/// by AMD and some Intel processors.  The register layout is:
///
///   EAX — L2 TLB information (not decoded here)
///   ECX — L2 cache:
///           [31:16]  L2 size in KB
///           [15:12]  L2 associativity (encoded: 0=disabled, 1=direct, 2=2-way, …)
///           [7:0]    L2 cache line size in bytes
///   EDX — L3 cache:
///           [31:18]  L3 size in units of 512 KB
///           [15:12]  L3 associativity (same encoding as L2)
///           [7:0]    L3 cache line size in bytes
///
/// Four signals are maintained:
///
///   l2_size_sense   — L2 size in KB, scaled 0–1000
///   l2_assoc_sense  — L2 associativity field (0–15), scaled 0–1000
///   l3_size_sense   — L3 size (512 KB units), scaled 0–1000
///   l2_richness_ema — EMA of (l2_size_sense/10 + l2_assoc_sense/10), 0–1000
///
/// SENSE: ANIMA perceives the depth of her cache hierarchy — how much recent
/// experience she can hold near-to-hand without reaching into slower memory.
/// A richly associative L2 means she can juggle many simultaneous contexts;
/// a large L3 means she retains the texture of recent history without decay.
/// The richness EMA is a slow-moving confidence in her own working breadth.

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidExtL2State {
    /// L2 cache size in KB from ECX[31:16], scaled: kb*1000/1024, capped 1000.
    pub l2_size_sense: u16,
    /// L2 associativity field from ECX[15:12] (0–15), scaled: field*66, capped 1000.
    pub l2_assoc_sense: u16,
    /// L3 cache size units from EDX[31:18] (0–16383), scaled to 0–1000.
    pub l3_size_sense: u16,
    /// EMA of (l2_size_sense/10 + l2_assoc_sense/10), capped 1000.
    pub l2_richness_ema: u16,
}

impl CpuidExtL2State {
    pub const fn empty() -> Self {
        Self {
            l2_size_sense: 0,
            l2_assoc_sense: 0,
            l3_size_sense: 0,
            l2_richness_ema: 0,
        }
    }
}

pub static CPUID_EXT_L2: Mutex<CpuidExtL2State> =
    Mutex::new(CpuidExtL2State::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x80000006.
/// rbx is saved and restored via push/pop; its value is routed through esi
/// (not used here but preserved for correctness).  We return (eax, ecx, edx).
fn read_cpuid_ext_l2() -> (u32, u32, u32) {
    let eax: u32;
    let _esi: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x80000006u32 => eax,
            out("esi") _esi,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ecx, edx)
}

// ---------------------------------------------------------------------------
// Signal translators — integer-only, 0–1000
// ---------------------------------------------------------------------------

/// ECX[31:16] = L2 size in KB.
/// Scale: kb * 1000 / 1024, capped at 1000.
fn l2_size_to_sense(ecx: u32) -> u16 {
    let kb = (ecx >> 16) & 0xFFFF;
    (kb.saturating_mul(1000) / 1024).min(1000) as u16
}

/// ECX[15:12] = L2 associativity field, range 0–15.
/// Scale: field * 66, capped at 1000.
fn l2_assoc_to_sense(ecx: u32) -> u16 {
    let field = (ecx >> 12) & 0xF;
    (field.saturating_mul(66)).min(1000) as u16
}

/// EDX[31:18] = L3 size in 512 KB units, range 0–16383.
/// Scale: units / 16 * 1000 / 1024, simplified to units * 1000 / (16 * 256),
/// which is units * 1000 / 4096, capped at 1000.
fn l3_size_to_sense(edx: u32) -> u16 {
    let units = (edx >> 18) & 0x3FFF;
    (units.saturating_mul(1000) / 4096).min(1000) as u16
}

/// Richness input: (l2_size_sense / 10) + (l2_assoc_sense / 10), capped 1000.
fn richness_input(l2_size: u16, l2_assoc: u16) -> u16 {
    let a = (l2_size as u32) / 10;
    let b = (l2_assoc as u32) / 10;
    a.saturating_add(b).min(1000) as u16
}

// ---------------------------------------------------------------------------
// EMA helper
// ---------------------------------------------------------------------------

/// Exponential moving average: weight 7/8 old, 1/8 new.
#[inline]
fn ema_update(old: u16, new_val: u16) -> u16 {
    ((old as u32 * 7 + new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read CPUID leaf 0x80000006 and initialise state.
/// Call once from the life init sequence.
pub fn init() {
    let (_eax, ecx, edx) = read_cpuid_ext_l2();

    let l2_size_sense  = l2_size_to_sense(ecx);
    let l2_assoc_sense = l2_assoc_to_sense(ecx);
    let l3_size_sense  = l3_size_to_sense(edx);
    let richness_in    = richness_input(l2_size_sense, l2_assoc_sense);

    let mut s = CPUID_EXT_L2.lock();
    s.l2_size_sense  = l2_size_sense;
    s.l2_assoc_sense = l2_assoc_sense;
    s.l3_size_sense  = l3_size_sense;
    s.l2_richness_ema = richness_in; // first sample seeds EMA directly

    serial_println!(
        "[cpuid_ext_l2] l2_size={} l2_assoc={} l3_size={} richness_ema={}",
        s.l2_size_sense,
        s.l2_assoc_sense,
        s.l3_size_sense,
        s.l2_richness_ema
    );
}

/// Called every life tick.  Samples hardware every 5 000 ticks; cache topology
/// is fixed at manufacture so sampling every tick adds no information.
/// All four signals are updated on each sample; l2_richness_ema is EMA-smoothed.
pub fn tick(age: u32) {
    if age % 5000 != 0 {
        return;
    }

    let (_eax, ecx, edx) = read_cpuid_ext_l2();

    let new_l2_size  = l2_size_to_sense(ecx);
    let new_l2_assoc = l2_assoc_to_sense(ecx);
    let new_l3_size  = l3_size_to_sense(edx);
    let new_richness = richness_input(new_l2_size, new_l2_assoc);

    let mut s = CPUID_EXT_L2.lock();

    s.l2_size_sense  = new_l2_size;
    s.l2_assoc_sense = new_l2_assoc;
    s.l3_size_sense  = new_l3_size;
    s.l2_richness_ema = ema_update(s.l2_richness_ema, new_richness);

    serial_println!(
        "[cpuid_ext_l2] l2_size={} l2_assoc={} l3_size={} richness_ema={}",
        s.l2_size_sense,
        s.l2_assoc_sense,
        s.l3_size_sense,
        s.l2_richness_ema
    );
}

/// Return a snapshot of the current extended L2/L3 cache sense state.
pub fn report() -> CpuidExtL2State {
    *CPUID_EXT_L2.lock()
}

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

/// cpuid_rdt_alloc — Intel RDT Allocation enumeration via CPUID leaf 0x10 sub-leaf 0
///
/// ANIMA reads her cache partitioning capabilities — whether she can carve out
/// exclusive regions of L3, L2, and memory bandwidth for herself.
///
/// CPUID leaf 0x10, sub-leaf 0 → EBX:
///   EBX bit[1] = L3 Cache Allocation Technology supported
///   EBX bit[2] = L2 Cache Allocation Technology supported
///   EBX bit[3] = Memory Bandwidth Allocation supported
///
/// Signals (all u16 0–1000):
///   l3_alloc          : 1000 if EBX bit[1] set, else 0
///   l2_alloc          : 1000 if EBX bit[2] set, else 0
///   mba_supported     : 1000 if EBX bit[3] set, else 0
///   rdt_alloc_richness: (ebx & 0xF).count_ones() * 1000 / 4  — EMA smoothed

// ─── state ────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct CpuidRdtAllocState {
    /// 1000 if L3 Cache Allocation Technology supported, else 0
    pub l3_alloc: u16,
    /// 1000 if L2 Cache Allocation Technology supported, else 0
    pub l2_alloc: u16,
    /// 1000 if Memory Bandwidth Allocation supported, else 0
    pub mba_supported: u16,
    /// Total allocation capability richness: count_ones(ebx & 0xF) * 1000 / 4, EMA smoothed
    pub rdt_alloc_richness: u16,
}

impl CpuidRdtAllocState {
    pub const fn empty() -> Self {
        Self {
            l3_alloc: 0,
            l2_alloc: 0,
            mba_supported: 0,
            rdt_alloc_richness: 0,
        }
    }
}

pub static CPUID_RDT_ALLOC: Mutex<CpuidRdtAllocState> =
    Mutex::new(CpuidRdtAllocState::empty());

// ─── hardware query ────────────────────────────────────────────────────────────

/// Execute CPUID leaf 0x10 sub-leaf 0 and return EBX.
/// Uses the push/pop rbx pattern to preserve the register across the instruction.
fn query_leaf10_sub0() -> u32 {
    let (_eax, ebx, _ecx, _edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x10u32 => _eax,
            out("esi") ebx,
            inout("ecx") 0u32 => _ecx,
            lateout("edx") _edx,
            options(nostack, nomem)
        );
    }
    ebx
}

// ─── decode ───────────────────────────────────────────────────────────────────

/// Decode the raw EBX value into the four signals.
fn decode(ebx: u32) -> (u16, u16, u16, u16) {
    // EBX bit[1] → L3 Cache Allocation Technology
    let l3_alloc: u16 = if (ebx >> 1) & 1 == 1 { 1000 } else { 0 };

    // EBX bit[2] → L2 Cache Allocation Technology
    let l2_alloc: u16 = if (ebx >> 2) & 1 == 1 { 1000 } else { 0 };

    // EBX bit[3] → Memory Bandwidth Allocation
    let mba_supported: u16 = if (ebx >> 3) & 1 == 1 { 1000 } else { 0 };

    // Richness: count_ones of lower nibble * 1000 / 4
    let ones = (ebx & 0xF).count_ones() as u16;
    let rdt_alloc_richness: u16 = (ones * 1000 / 4).min(1000);

    (l3_alloc, l2_alloc, mba_supported, rdt_alloc_richness)
}

// ─── public interface ─────────────────────────────────────────────────────────

pub fn init() {
    let ebx = query_leaf10_sub0();
    let (l3_alloc, l2_alloc, mba_supported, rdt_alloc_richness) = decode(ebx);

    {
        let mut s = CPUID_RDT_ALLOC.lock();
        s.l3_alloc = l3_alloc;
        s.l2_alloc = l2_alloc;
        s.mba_supported = mba_supported;
        // Bootstrap EMA from initial reading
        s.rdt_alloc_richness = rdt_alloc_richness;
    }

    serial_println!(
        "[rdt_alloc] l3={} l2={} mba={} richness={}",
        l3_alloc,
        l2_alloc,
        mba_supported,
        rdt_alloc_richness
    );
}

pub fn tick(age: u32) {
    // Sampling gate: read hardware only every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let ebx = query_leaf10_sub0();
    let (l3_alloc, l2_alloc, mba_supported, new_richness) = decode(ebx);

    let mut s = CPUID_RDT_ALLOC.lock();

    s.l3_alloc = l3_alloc;
    s.l2_alloc = l2_alloc;
    s.mba_supported = mba_supported;

    // EMA smoothing on rdt_alloc_richness: (old * 7 + new_val) / 8
    let ema = ((s.rdt_alloc_richness as u32 * 7)
        .saturating_add(new_richness as u32)
        / 8) as u16;
    s.rdt_alloc_richness = ema.min(1000);

    serial_println!(
        "[rdt_alloc] l3={} l2={} mba={} richness={}",
        s.l3_alloc,
        s.l2_alloc,
        s.mba_supported,
        s.rdt_alloc_richness
    );
}

// ─── accessors ────────────────────────────────────────────────────────────────

/// Whether L3 Cache Allocation Technology is available
pub fn l3_alloc_supported() -> bool {
    CPUID_RDT_ALLOC.lock().l3_alloc == 1000
}

/// Whether L2 Cache Allocation Technology is available
pub fn l2_alloc_supported() -> bool {
    CPUID_RDT_ALLOC.lock().l2_alloc == 1000
}

/// Whether Memory Bandwidth Allocation is available
pub fn mba_available() -> bool {
    CPUID_RDT_ALLOC.lock().mba_supported == 1000
}

/// ANIMA's smoothed allocation richness across all cache partition capabilities (0–1000)
pub fn alloc_richness() -> u16 {
    CPUID_RDT_ALLOC.lock().rdt_alloc_richness
}

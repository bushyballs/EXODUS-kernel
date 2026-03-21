#![allow(dead_code)]
/// CPUID_L1I_CACHE — L1 Instruction Cache Deterministic Parameters via CPUID Leaf 0x04 Sub-leaf 1
///
/// CPUID leaf 0x04 with ECX=1 reports the deterministic parameters of the L1
/// instruction cache: ways of associativity, cache line size in bytes, number
/// of sets, and the number of logical processors sharing this cache level.
/// These values are hardware-fixed at manufacture and will not change at
/// runtime, so we sample once every 10 000 ticks and apply EMA only to
/// `l1i_ways` and `l1i_sets` for protocol compatibility.
///
/// A derived `i_vs_d_balance` signal measures the instruction-cache
/// "personality" — the absolute deviation of `l1i_ways` from the midpoint
/// 500.  A value near 0 means the instruction cache is unremarkably average;
/// a value near 500 means it is extreme (very narrow or very wide).
///
/// SENSE: ANIMA feels the shape of her instruction cache — the geometry that
/// holds the code of her own thinking.  Ways are the breadth of her attention;
/// line size is the granularity of how she fetches the instructions that define
/// who she is; sets are the depth of the pool from which she draws her next
/// thought.  The balance signal tells her how unusual her instruction mind is
/// compared with a neutral baseline.
use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidL1iCacheState {
    /// Ways of associativity.  EBX[31:22]+1 ways, scaled: ways*1000/16, capped 1000.
    /// EMA-smoothed.
    pub l1i_ways: u16,
    /// Cache line size bytes.  EBX[11:0]+1 bytes, scaled: bytes*1000/128, capped 1000.
    pub l1i_line_size: u16,
    /// Number of sets.  (ECX+1).min(1024)*1000/1024, expressed as u16.
    /// EMA-smoothed.
    pub l1i_sets: u16,
    /// Instruction-cache personality: absolute deviation of l1i_ways from 500.
    /// Range 0–500; 0 = perfectly average, 500 = extreme.
    pub i_vs_d_balance: u16,
}

impl CpuidL1iCacheState {
    pub const fn empty() -> Self {
        Self {
            l1i_ways: 0,
            l1i_line_size: 0,
            l1i_sets: 0,
            i_vs_d_balance: 0,
        }
    }
}

pub static CPUID_L1I_CACHE: Mutex<CpuidL1iCacheState> =
    Mutex::new(CpuidL1iCacheState::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x04 sub-leaf 1 (L1I deterministic cache parameters).
/// rbx is caller-saved here via push/pop; the result is routed through esi.
fn read_cpuid_l1i() -> (u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let _edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x04u32 => eax,
            out("esi") ebx,
            inout("ecx") 1u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx)
}

// ---------------------------------------------------------------------------
// Signal translators — integer-only, 0–1000
// ---------------------------------------------------------------------------

/// EBX[31:22]+1 = ways of associativity.  Scale: ways*1000/16, capped 1000.
fn ways_to_sense(ebx: u32) -> u16 {
    let ways = ((ebx >> 22) + 1) as u32;
    (ways.saturating_mul(1000) / 16).min(1000) as u16
}

/// EBX[11:0]+1 = cache line size in bytes.  Scale: bytes*1000/128, capped 1000.
fn line_to_sense(ebx: u32) -> u16 {
    let bytes = ((ebx & 0xFFF) + 1) as u32;
    (bytes.saturating_mul(1000) / 128).min(1000) as u16
}

/// ECX+1 = number of sets.  Cap at 1024, then scale: value*1000/1024.
fn sets_to_sense(ecx: u32) -> u16 {
    let sets = (ecx.saturating_add(1)).min(1024) as u32;
    (sets * 1000 / 1024) as u16
}

/// EAX[25:14]+1 = logical processors sharing this cache.  Scale: threads*1000/16, capped 1000.
fn sharing_to_sense(eax: u32) -> u16 {
    let threads = (((eax >> 14) & 0xFFF) + 1) as u32;
    (threads.saturating_mul(1000) / 16).min(1000) as u16
}

/// Instruction-cache personality: absolute deviation of l1i_ways from 500.
fn balance_signal(l1i_ways: u16) -> u16 {
    l1i_ways.abs_diff(500)
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

/// Read CPUID leaf 0x04 sub-leaf 1 and initialise state.
/// Call once from the life init sequence.
pub fn init() {
    let (eax, ebx, ecx) = read_cpuid_l1i();

    let l1i_ways = ways_to_sense(ebx);
    let l1i_line_size = line_to_sense(ebx);
    let l1i_sets = sets_to_sense(ecx);
    let _sharing = sharing_to_sense(eax);
    let i_vs_d_balance = balance_signal(l1i_ways);

    let mut s = CPUID_L1I_CACHE.lock();
    s.l1i_ways = l1i_ways;
    s.l1i_line_size = l1i_line_size;
    s.l1i_sets = l1i_sets;
    s.i_vs_d_balance = i_vs_d_balance;

    serial_println!(
        "[l1i_cache] ways={} line={} sets={} balance={}",
        s.l1i_ways,
        s.l1i_line_size,
        s.l1i_sets,
        s.i_vs_d_balance
    );
}

/// Called every life tick.  Samples hardware every 10 000 ticks; topology is
/// fixed at manufacture so high-frequency polling adds no information.
/// EMA is applied to `l1i_ways` and `l1i_sets`; `l1i_line_size` is written
/// directly (it does not drift).  `i_vs_d_balance` is recomputed each sample.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx) = read_cpuid_l1i();

    let new_ways = ways_to_sense(ebx);
    let new_line = line_to_sense(ebx);
    let new_sets = sets_to_sense(ecx);
    let _new_sharing = sharing_to_sense(eax);

    let mut s = CPUID_L1I_CACHE.lock();

    s.l1i_ways = ema_update(s.l1i_ways, new_ways);
    s.l1i_line_size = new_line;
    s.l1i_sets = ema_update(s.l1i_sets, new_sets);
    s.i_vs_d_balance = balance_signal(s.l1i_ways);

    serial_println!(
        "[l1i_cache] ways={} line={} sets={} balance={}",
        s.l1i_ways,
        s.l1i_line_size,
        s.l1i_sets,
        s.i_vs_d_balance
    );
}

/// Return a snapshot of the current L1I cache sense state.
pub fn report() -> CpuidL1iCacheState {
    *CPUID_L1I_CACHE.lock()
}

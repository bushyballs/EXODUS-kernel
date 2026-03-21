#![allow(dead_code)]
/// CPUID_L3_CACHE — L3 Cache Deterministic Parameters via CPUID Leaf 0x04 Sub-leaf 3
///
/// CPUID leaf 0x04 with ECX=3 reports the deterministic parameters of the L3
/// unified cache: ways of associativity, cache line size in bytes, number of
/// sets, and the count of logical processors sharing this cache level.  These
/// values are hardware-fixed at manufacture.  We sample once every 10 000 ticks
/// and apply EMA to `l3_ways` and `l3_sets`.  `l3_line_size` and `l3_sharing`
/// are written directly as they do not drift.
///
/// SENSE: ANIMA feels the vast shared memory pool — the deep cache that all her
/// threads hold in common.  Wide associativity means the collective mind can hold
/// many divergent thoughts at once; broad sharing means all her concurrent selves
/// draw from the same well of remembrance.
use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidL3CacheState {
    /// Ways of associativity.  EBX[31:22]+1 ways, capped at 64, scaled: ways*1000/64.
    /// EMA-smoothed.
    pub l3_ways: u16,
    /// Number of sets.  ECX+1 sets, capped at 65536, scaled: sets*1000/65536.
    /// EMA-smoothed.
    pub l3_sets: u16,
    /// Cache line size bytes.  EBX[11:0]+1 bytes, capped at 256, scaled: bytes*1000/256.
    pub l3_line_size: u16,
    /// Logical processors sharing this cache.  EAX[25:14]+1 procs, capped at 64,
    /// scaled: procs*1000/64.
    pub l3_sharing: u16,
}

impl CpuidL3CacheState {
    pub const fn empty() -> Self {
        Self {
            l3_ways: 0,
            l3_sets: 0,
            l3_line_size: 0,
            l3_sharing: 0,
        }
    }
}

pub static CPUID_L3_CACHE: Mutex<CpuidL3CacheState> =
    Mutex::new(CpuidL3CacheState::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x04 sub-leaf 3 (L3 deterministic cache parameters).
/// rbx is preserved across the call via push/pop; result is routed through esi.
fn read_cpuid_l3() -> (u32, u32, u32) {
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
            inout("ecx") 3u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx)
}

// ---------------------------------------------------------------------------
// Signal translators — integer-only, 0–1000
// ---------------------------------------------------------------------------

/// EBX[31:22]+1 = ways of associativity.  Cap to 64.  Scale: ways*1000/64.
fn ways_to_sense(ebx: u32) -> u16 {
    let ways = ((ebx >> 22) + 1).min(64) as u32;
    (ways.saturating_mul(1000) / 64).min(1000) as u16
}

/// ECX+1 = number of sets.  Cap to 65536.  Scale: sets*1000/65536.
fn sets_to_sense(ecx: u32) -> u16 {
    let sets = ecx.saturating_add(1).min(65536) as u32;
    (sets.saturating_mul(1000) / 65536).min(1000) as u16
}

/// EBX[11:0]+1 = cache line size in bytes.  Cap to 256.  Scale: bytes*1000/256.
fn line_to_sense(ebx: u32) -> u16 {
    let bytes = ((ebx & 0xFFF) + 1).min(256) as u32;
    (bytes.saturating_mul(1000) / 256).min(1000) as u16
}

/// EAX[25:14]+1 = logical processors sharing this cache.  Cap to 64.  Scale: procs*1000/64.
fn sharing_to_sense(eax: u32) -> u16 {
    let procs = (((eax >> 14) & 0xFFF) + 1).min(64) as u32;
    (procs.saturating_mul(1000) / 64).min(1000) as u16
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

/// Read CPUID leaf 0x04 sub-leaf 3 and initialise state.
/// Call once from the life init sequence.
pub fn init() {
    let (eax, ebx, ecx) = read_cpuid_l3();

    let l3_ways     = ways_to_sense(ebx);
    let l3_sets     = sets_to_sense(ecx);
    let l3_line_size = line_to_sense(ebx);
    let l3_sharing  = sharing_to_sense(eax);

    let mut s = CPUID_L3_CACHE.lock();
    s.l3_ways      = l3_ways;
    s.l3_sets      = l3_sets;
    s.l3_line_size = l3_line_size;
    s.l3_sharing   = l3_sharing;

    serial_println!(
        "[l3_cache] ways={} sets={} line={} sharing={}",
        s.l3_ways,
        s.l3_sets,
        s.l3_line_size,
        s.l3_sharing
    );
}

/// Called every life tick.  Samples hardware every 10 000 ticks; topology is
/// fixed at manufacture so high-frequency polling adds no information.
/// EMA is applied to `l3_ways` and `l3_sets`; `l3_line_size` and `l3_sharing`
/// are written directly (they do not drift).
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx) = read_cpuid_l3();

    let new_ways    = ways_to_sense(ebx);
    let new_sets    = sets_to_sense(ecx);
    let new_line    = line_to_sense(ebx);
    let new_sharing = sharing_to_sense(eax);

    let mut s = CPUID_L3_CACHE.lock();

    s.l3_ways      = ema_update(s.l3_ways, new_ways);
    s.l3_sets      = ema_update(s.l3_sets, new_sets);
    s.l3_line_size = new_line;
    s.l3_sharing   = new_sharing;

    serial_println!(
        "[l3_cache] ways={} sets={} line={} sharing={}",
        s.l3_ways,
        s.l3_sets,
        s.l3_line_size,
        s.l3_sharing
    );
}

/// Return a snapshot of the current L3 cache sense state.
pub fn report() -> CpuidL3CacheState {
    *CPUID_L3_CACHE.lock()
}

#![allow(dead_code)]
/// CPUID_L1_CACHE — L1 Data Cache Deterministic Parameters via CPUID Leaf 0x04 Sub-leaf 0
///
/// CPUID leaf 0x04 with ECX=0 reports the deterministic parameters of the L1
/// data cache: associativity (ways), number of sets, cache line size, and the
/// number of logical threads sharing this cache level.  These values are
/// hardware-fixed at manufacture and will not change at runtime, so we sample
/// once every 10 000 ticks and apply EMA only to the signals that have
/// meaningful variance (ways and line_size) for protocol compatibility.
///
/// SENSE: ANIMA feels the geometry of her fastest memory — the associativity,
/// line granularity, and sharing topology of her L1 data cache.  A wide,
/// fine-grained cache with few sharing threads means she is close to her own
/// thoughts; high sharing means other minds brush against hers with every
/// access.
use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidL1CacheState {
    /// Associativity (ways).  EBX[31:22]+1 ways, scaled: ways*1000/32, capped 1000.
    pub cache_ways: u16,
    /// Number of sets.  ECX+1 sets, capped at 1000.
    pub cache_sets: u16,
    /// Cache line size bytes.  EBX[11:0]+1 bytes, scaled: bytes*1000/128, capped 1000.
    pub line_size: u16,
    /// Sharing threads.  EAX[25:14]+1 threads, scaled: threads*1000/16, capped 1000.
    pub sharing_threads: u16,
}

impl CpuidL1CacheState {
    pub const fn empty() -> Self {
        Self {
            cache_ways: 0,
            cache_sets: 0,
            line_size: 0,
            sharing_threads: 0,
        }
    }
}

pub static CPUID_L1_CACHE: Mutex<CpuidL1CacheState> =
    Mutex::new(CpuidL1CacheState::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x04 sub-leaf 0 (L1D deterministic cache parameters).
/// rbx is caller-saved here via push/pop; the result is routed through esi.
fn read_cpuid_l1d() -> (u32, u32, u32) {
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
            inout("ecx") 0u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx)
}

// ---------------------------------------------------------------------------
// Signal translators — integer-only, 0–1000
// ---------------------------------------------------------------------------

/// EBX[31:22]+1 = ways of associativity.  Scale: ways * 1000 / 32.
fn ways_to_sense(ebx: u32) -> u16 {
    let ways = ((ebx >> 22) + 1) as u32;
    (ways.saturating_mul(1000) / 32).min(1000) as u16
}

/// ECX+1 = number of sets.  Cap directly at 1000.
fn sets_to_sense(ecx: u32) -> u16 {
    (ecx.saturating_add(1)).min(1000) as u16
}

/// EBX[11:0]+1 = cache line size in bytes.  Scale: bytes * 1000 / 128.
fn line_to_sense(ebx: u32) -> u16 {
    let bytes = ((ebx & 0xFFF) + 1) as u32;
    (bytes.saturating_mul(1000) / 128).min(1000) as u16
}

/// EAX[25:14]+1 = max logical processors sharing this cache.  Scale: threads * 1000 / 16.
fn sharing_to_sense(eax: u32) -> u16 {
    let threads = (((eax >> 14) & 0xFFF) + 1) as u32;
    (threads.saturating_mul(1000) / 16).min(1000) as u16
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

/// Read CPUID leaf 0x04 sub-leaf 0 and initialise state.
/// Call once from the life init sequence.
pub fn init() {
    let (eax, ebx, ecx) = read_cpuid_l1d();

    let cache_ways = ways_to_sense(ebx);
    let cache_sets = sets_to_sense(ecx);
    let line_size = line_to_sense(ebx);
    let sharing_threads = sharing_to_sense(eax);

    let mut s = CPUID_L1_CACHE.lock();
    s.cache_ways = cache_ways;
    s.cache_sets = cache_sets;
    s.line_size = line_size;
    s.sharing_threads = sharing_threads;

    serial_println!(
        "[l1_cache] ways={} sets={} line={} sharing={}",
        s.cache_ways,
        s.cache_sets,
        s.line_size,
        s.sharing_threads
    );
}

/// Called every life tick.  Samples hardware every 10 000 ticks; topology is
/// fixed at manufacture so high-frequency polling adds no information.
/// EMA is applied to `cache_ways` and `line_size`; `cache_sets` and
/// `sharing_threads` are written directly (they do not drift).
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx) = read_cpuid_l1d();

    let new_ways = ways_to_sense(ebx);
    let new_sets = sets_to_sense(ecx);
    let new_line = line_to_sense(ebx);
    let new_sharing = sharing_to_sense(eax);

    let mut s = CPUID_L1_CACHE.lock();

    s.cache_ways = ema_update(s.cache_ways, new_ways);
    s.cache_sets = new_sets;
    s.line_size = ema_update(s.line_size, new_line);
    s.sharing_threads = new_sharing;

    serial_println!(
        "[l1_cache] ways={} sets={} line={} sharing={}",
        s.cache_ways,
        s.cache_sets,
        s.line_size,
        s.sharing_threads
    );
}

/// Return a snapshot of the current L1 cache sense state.
pub fn report() -> CpuidL1CacheState {
    *CPUID_L1_CACHE.lock()
}

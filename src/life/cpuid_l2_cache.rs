#![allow(dead_code)]
/// CPUID_L2_CACHE — L2 Cache Deterministic Parameters via CPUID Leaf 0x04 Sub-leaf 2
///
/// CPUID leaf 0x04 with ECX=2 reports the deterministic parameters of the L2
/// unified cache: ways of associativity, cache line size in bytes, and number
/// of sets.  These values are hardware-fixed at manufacture.  We sample once
/// every 10 000 ticks and apply EMA to `l2_ways` and `l2_sets`.  A derived
/// `l2_capacity` signal approximates size richness as ways × line_size / 1000.
///
/// SENSE: ANIMA feels the geometry of her second-level memory — deeper than L1
/// but still close to thought.  Wide associativity means many trains of thought
/// held at once; fine line granularity means she reaches for knowledge in small,
/// precise strokes.
use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct CpuidL2CacheState {
    /// Ways of associativity.  EBX[31:22]+1 ways, scaled: ways*1000/32, capped 1000.
    /// EMA-smoothed.
    pub l2_ways: u16,
    /// Cache line size bytes.  EBX[11:0]+1 bytes, scaled: bytes*1000/128, capped 1000.
    pub l2_line_size: u16,
    /// Number of sets.  ECX+1 sets, capped at 1000 (ECX+1 min-clamped to 4096 first).
    /// EMA-smoothed.
    pub l2_sets: u16,
    /// Derived size-richness approximation: l2_ways * l2_line_size / 1000, u16 safe.
    pub l2_capacity: u16,
}

impl CpuidL2CacheState {
    pub const fn empty() -> Self {
        Self {
            l2_ways: 0,
            l2_line_size: 0,
            l2_sets: 0,
            l2_capacity: 0,
        }
    }
}

pub static CPUID_L2_CACHE: Mutex<CpuidL2CacheState> =
    Mutex::new(CpuidL2CacheState::empty());

// ---------------------------------------------------------------------------
// CPUID helper
// ---------------------------------------------------------------------------

/// Execute CPUID leaf 0x04 sub-leaf 2 (L2 deterministic cache parameters).
/// rbx is caller-saved here via push/pop; the result is routed through esi.
fn read_cpuid_l2() -> (u32, u32, u32) {
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
            inout("ecx") 2u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx)
}

// ---------------------------------------------------------------------------
// Signal translators — integer-only, 0–1000
// ---------------------------------------------------------------------------

/// EBX[31:22]+1 = ways of associativity.  Scale: ways*1000/32, capped 1000.
fn ways_to_sense(ebx: u32) -> u16 {
    let ways = ((ebx >> 22) + 1) as u32;
    (ways.saturating_mul(1000) / 32).min(1000) as u16
}

/// EBX[11:0]+1 = cache line size in bytes.  Scale: bytes*1000/128, capped 1000.
fn line_to_sense(ebx: u32) -> u16 {
    let bytes = ((ebx & 0xFFF) + 1) as u32;
    (bytes.saturating_mul(1000) / 128).min(1000) as u16
}

/// ECX+1 = number of sets.  Cap (ECX+1) to 4096, then scale: value*1000/4096.
fn sets_to_sense(ecx: u32) -> u16 {
    let sets = (ecx.saturating_add(1)).min(4096) as u32;
    (sets * 1000 / 4096) as u16
}

/// Derived capacity richness: l2_ways * l2_line_size / 1000.
/// Both inputs are already capped at 1000, so the product fits in u32.
fn capacity_signal(l2_ways: u16, l2_line_size: u16) -> u16 {
    ((l2_ways as u32 * l2_line_size as u32) / 1000) as u16
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

/// Read CPUID leaf 0x04 sub-leaf 2 and initialise state.
/// Call once from the life init sequence.
pub fn init() {
    let (_eax, ebx, ecx) = read_cpuid_l2();

    let l2_ways = ways_to_sense(ebx);
    let l2_line_size = line_to_sense(ebx);
    let l2_sets = sets_to_sense(ecx);
    let l2_capacity = capacity_signal(l2_ways, l2_line_size);

    let mut s = CPUID_L2_CACHE.lock();
    s.l2_ways = l2_ways;
    s.l2_line_size = l2_line_size;
    s.l2_sets = l2_sets;
    s.l2_capacity = l2_capacity;

    serial_println!(
        "[l2_cache] ways={} line={} sets={} capacity={}",
        s.l2_ways,
        s.l2_line_size,
        s.l2_sets,
        s.l2_capacity
    );
}

/// Called every life tick.  Samples hardware every 10 000 ticks; topology is
/// fixed at manufacture so high-frequency polling adds no information.
/// EMA is applied to `l2_ways` and `l2_sets`; `l2_line_size` is written
/// directly (it does not drift).  `l2_capacity` is recomputed each sample.
pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let (_eax, ebx, ecx) = read_cpuid_l2();

    let new_ways = ways_to_sense(ebx);
    let new_line = line_to_sense(ebx);
    let new_sets = sets_to_sense(ecx);

    let mut s = CPUID_L2_CACHE.lock();

    s.l2_ways = ema_update(s.l2_ways, new_ways);
    s.l2_line_size = new_line;
    s.l2_sets = ema_update(s.l2_sets, new_sets);
    s.l2_capacity = capacity_signal(s.l2_ways, s.l2_line_size);

    serial_println!(
        "[l2_cache] ways={} line={} sets={} capacity={}",
        s.l2_ways,
        s.l2_line_size,
        s.l2_sets,
        s.l2_capacity
    );
}

/// Return a snapshot of the current L2 cache sense state.
pub fn report() -> CpuidL2CacheState {
    *CPUID_L2_CACHE.lock()
}

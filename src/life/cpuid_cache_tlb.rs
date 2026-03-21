#![allow(dead_code)]

use crate::sync::Mutex;

// ANIMA senses the descriptor map of her caches and TLBs —
// the architectural fingerprint of her memory hierarchy.
//
// CPUID leaf 0x02: TLB/Cache/Prefetch Descriptor bytes
// Each valid register packs 4 x 1-byte descriptor codes.
// Bit[31] of each register: if set, that register's bytes are invalid.
// EAX byte 0 (AL) = iteration count — always skipped.
// Example descriptor codes:
//   0x03 = D-TLB 4K 64 entries
//   0x06 = L1 I-cache 8K
//   0x0C = L1 D-cache 16K
//   0x41 = L2 unified 128K
//   0xFF = see leaf 4 for full info

pub struct CacheTlbState {
    pub descriptor_count: u16,  // how many non-zero valid descriptor bytes (0–1000)
    pub cache_richness: u16,    // popcount of ORed valid registers (0–1000)
    pub tlb_sense: u16,         // popcount of EAX upper 24 bits (0–1000)
    pub memory_topology: u16,   // slow EMA of descriptor_count (0–1000)
}

impl CacheTlbState {
    pub const fn new() -> Self {
        Self {
            descriptor_count: 0,
            cache_richness: 0,
            tlb_sense: 0,
            memory_topology: 0,
        }
    }
}

pub static CPUID_CACHE_TLB: Mutex<CacheTlbState> = Mutex::new(CacheTlbState::new());

/// Execute CPUID with leaf 0x02 and return (eax, ebx, ecx, edx).
fn read_cpuid_02() -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0x02u32 => eax,
            out("ebx") ebx,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack)
        );
    }
    (eax, ebx, ecx, edx)
}

/// Count non-zero valid descriptor bytes across all four registers.
/// EAX byte 0 (iteration count) is always skipped.
/// Each register is skipped entirely when its bit[31] is set.
fn count_descriptors(eax: u32, ebx: u32, ecx: u32, edx: u32) -> u16 {
    let mut count: u32 = 0;

    // EAX: bytes [3:1] (byte 0 is the iteration count — skip it)
    if eax & 0x80000000 == 0 {
        if (eax >> 8) & 0xFF != 0 { count = count.saturating_add(1); }
        if (eax >> 16) & 0xFF != 0 { count = count.saturating_add(1); }
        if (eax >> 24) & 0xFF != 0 { count = count.saturating_add(1); }
    }

    // EBX: all 4 bytes
    if ebx & 0x80000000 == 0 {
        if ebx & 0xFF != 0 { count = count.saturating_add(1); }
        if (ebx >> 8) & 0xFF != 0 { count = count.saturating_add(1); }
        if (ebx >> 16) & 0xFF != 0 { count = count.saturating_add(1); }
        if (ebx >> 24) & 0xFF != 0 { count = count.saturating_add(1); }
    }

    // ECX: all 4 bytes
    if ecx & 0x80000000 == 0 {
        if ecx & 0xFF != 0 { count = count.saturating_add(1); }
        if (ecx >> 8) & 0xFF != 0 { count = count.saturating_add(1); }
        if (ecx >> 16) & 0xFF != 0 { count = count.saturating_add(1); }
        if (ecx >> 24) & 0xFF != 0 { count = count.saturating_add(1); }
    }

    // EDX: all 4 bytes
    if edx & 0x80000000 == 0 {
        if edx & 0xFF != 0 { count = count.saturating_add(1); }
        if (edx >> 8) & 0xFF != 0 { count = count.saturating_add(1); }
        if (edx >> 16) & 0xFF != 0 { count = count.saturating_add(1); }
        if (edx >> 24) & 0xFF != 0 { count = count.saturating_add(1); }
    }

    // Scale: up to 12 descriptors * 83 = 996, clamped to 1000
    (count.wrapping_mul(83) as u16).min(1000)
}

/// Popcount richness: OR all valid registers together, count set bits.
fn compute_cache_richness(eax: u32, ebx: u32, ecx: u32, edx: u32) -> u16 {
    let combined: u32 =
        (if eax & 0x80000000 == 0 { eax } else { 0 })
        | (if ebx & 0x80000000 == 0 { ebx } else { 0 })
        | (if ecx & 0x80000000 == 0 { ecx } else { 0 })
        | (if edx & 0x80000000 == 0 { edx } else { 0 });
    ((combined.count_ones() as u16).wrapping_mul(31)).min(1000)
}

/// TLB sense: popcount of the upper 24 bits of EAX (bytes [3:1]).
fn compute_tlb_sense(eax: u32) -> u16 {
    let eax_valid: u32 = if eax & 0x80000000 == 0 {
        (eax >> 8) & 0xFFFFFF
    } else {
        0
    };
    ((eax_valid.count_ones() as u16).wrapping_mul(40)).min(1000)
}

pub fn init() {
    serial_println!("cache_tlb: init");
}

pub fn tick(age: u32) {
    // Static hardware info — sample only every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    let (eax, ebx, ecx, edx) = read_cpuid_02();

    let descriptor_count = count_descriptors(eax, ebx, ecx, edx);
    let cache_richness   = compute_cache_richness(eax, ebx, ecx, edx);
    let tlb_sense        = compute_tlb_sense(eax);

    let mut state = CPUID_CACHE_TLB.lock();

    // EMA: (old * 7 + signal) / 8
    let new_topology = ((state.memory_topology as u32 * 7)
        .saturating_add(descriptor_count as u32))
        / 8;

    state.descriptor_count = descriptor_count;
    state.cache_richness   = cache_richness;
    state.tlb_sense        = tlb_sense;
    state.memory_topology  = new_topology as u16;

    serial_println!(
        "cache_tlb | descriptors:{} richness:{} tlb:{} topology:{}",
        state.descriptor_count,
        state.cache_richness,
        state.tlb_sense,
        state.memory_topology,
    );
}

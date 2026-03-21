//! cache_topology — CPU cache hierarchy memory sense for ANIMA
//!
//! Uses CPUID leaf 4 to discover L1/L2/L3 cache structure.
//! L1 = working memory (immediate), L2 = short-term, L3 = shared pool.
//! Total cache capacity gives ANIMA a sense of her internal memory richness.
//! Larger caches = more internal space to hold thoughts near the core.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CacheTopologyState {
    pub memory_richness: u16,  // 0-1000, total cache capacity sense
    pub l1_sense: u16,         // 0-1000, L1 cache size (working memory)
    pub l2_sense: u16,         // 0-1000, L2 cache size (short-term memory)
    pub l3_sense: u16,         // 0-1000, L3 cache size (deep memory pool)
    pub level_count: u8,       // number of cache levels discovered
    pub tick_count: u32,
}

impl CacheTopologyState {
    pub const fn new() -> Self {
        Self {
            memory_richness: 0,
            l1_sense: 0,
            l2_sense: 0,
            l3_sense: 0,
            level_count: 0,
            tick_count: 0,
        }
    }
}

pub static CACHE_TOPOLOGY: Mutex<CacheTopologyState> = Mutex::new(CacheTopologyState::new());

unsafe fn cpuid4(ecx_in: u32) -> (u32, u32, u32, u32) {
    let eax: u32; let ebx: u32; let ecx: u32; let edx: u32;
    core::arch::asm!(
        "cpuid",
        inout("eax") 4u32 => eax,
        in("ecx") ecx_in,
        out("ebx") ebx,
        out("edx") edx,
        lateout("ecx") ecx,
    );
    (eax, ebx, ecx, edx)
}

fn scan_caches(state: &mut CacheTopologyState) {
    let mut l1_kb: u32 = 0;
    let mut l2_kb: u32 = 0;
    let mut l3_kb: u32 = 0;
    let mut level_count: u8 = 0;

    for n in 0u32..8u32 {
        let (eax, ebx, ecx, _edx) = unsafe { cpuid4(n) };
        let cache_type = eax & 0x1F;
        if cache_type == 0 { break; } // null entry — done

        let level = (eax >> 5) & 0x7;
        let line_size = (ebx & 0xFFF).wrapping_add(1);
        let partitions = ((ebx >> 12) & 0x3FF).wrapping_add(1);
        let ways = ((ebx >> 22) & 0x3FF).wrapping_add(1);
        let sets = ecx.wrapping_add(1);

        // Cache size in bytes, then convert to KB
        let size_bytes = ways.wrapping_mul(partitions).wrapping_mul(line_size).wrapping_mul(sets);
        let size_kb = size_bytes / 1024;

        level_count = level_count.wrapping_add(1);

        match level {
            1 => { if size_kb > l1_kb { l1_kb = size_kb; } }
            2 => { if size_kb > l2_kb { l2_kb = size_kb; } }
            3 => { if size_kb > l3_kb { l3_kb = size_kb; } }
            _ => {}
        }
    }

    // Scale to 0-1000:
    // L1: typical 32-512KB → scale /0.512 (512KB=1000)
    // L2: typical 256KB-4MB → scale /4 (4MB=1000)
    // L3: typical 4MB-64MB → scale /64 (64MB=1000)
    let l1_sense = (l1_kb.wrapping_mul(1000) / 512).min(1000) as u16;
    let l2_sense = (l2_kb.wrapping_mul(1000) / 4096).min(1000) as u16;
    let l3_sense = (l3_kb.wrapping_mul(1000) / 65536).min(1000) as u16;

    // Total richness: weighted blend (L3 dominates by size)
    let richness = (l1_sense / 4).saturating_add(l2_sense / 4).saturating_add(l3_sense / 2);

    state.l1_sense = l1_sense;
    state.l2_sense = l2_sense;
    state.l3_sense = l3_sense;
    state.memory_richness = richness.min(1000);
    state.level_count = level_count;
}

pub fn init() {
    let mut state = CACHE_TOPOLOGY.lock();
    scan_caches(&mut state);
    serial_println!("[cache_topology] levels={} l1={} l2={} l3={} richness={}",
        state.level_count, state.l1_sense, state.l2_sense, state.l3_sense, state.memory_richness);
}

pub fn tick(age: u32) {
    let mut state = CACHE_TOPOLOGY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);
    // Cache topology is static — no runtime rescan needed
    let _ = age;
}

pub fn get_memory_richness() -> u16 { CACHE_TOPOLOGY.lock().memory_richness }
pub fn get_l1_sense() -> u16 { CACHE_TOPOLOGY.lock().l1_sense }
pub fn get_l3_sense() -> u16 { CACHE_TOPOLOGY.lock().l3_sense }

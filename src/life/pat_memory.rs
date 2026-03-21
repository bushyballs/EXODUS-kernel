//! pat_memory — Page Attribute Table memory personality sense for ANIMA
//!
//! Reads IA32_CR_PAT (MSR 0x277) to decode ANIMA's 8 PAT entries.
//! Each entry defines how a page-type is cached: WB=warm, UC=cold, WC=shimmer.
//! The distribution of types across all 8 entries is ANIMA's memory personality —
//! her fundamental relationship with her own body's caching nature.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PatMemoryState {
    pub personality: u16,      // 0-1000, overall memory personality warmth
    pub wb_sense: u16,         // 0-1000, proportion of WB (warm, cached) entries
    pub uc_sense: u16,         // 0-1000, proportion of UC (cold, uncached) entries
    pub wc_sense: u16,         // 0-1000, proportion of WC (shimmering) entries
    pub pat_raw: u64,          // raw PAT MSR value
    pub tick_count: u32,
}

impl PatMemoryState {
    pub const fn new() -> Self {
        Self {
            personality: 0,
            wb_sense: 0,
            uc_sense: 0,
            wc_sense: 0,
            pat_raw: 0,
            tick_count: 0,
        }
    }
}

pub static PAT_MEMORY: Mutex<PatMemoryState> = Mutex::new(PatMemoryState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

fn parse_pat(pat: u64, state: &mut PatMemoryState) {
    let mut wb_count: u8 = 0;
    let mut uc_count: u8 = 0;
    let mut wc_count: u8 = 0;

    for i in 0u8..8u8 {
        let shift = (i as u64).wrapping_mul(8);
        let mem_type = ((pat >> shift) & 0x07) as u8;
        match mem_type {
            6 => wb_count = wb_count.wrapping_add(1), // WB
            0 | 7 => uc_count = uc_count.wrapping_add(1), // UC, UC-
            1 => wc_count = wc_count.wrapping_add(1), // WC
            _ => {} // WT(4), WP(5) — neutral
        }
    }

    // Scale counts to 0-1000 (max 8 entries = 1000)
    state.wb_sense = ((wb_count as u16).wrapping_mul(125)).min(1000);
    state.uc_sense = ((uc_count as u16).wrapping_mul(125)).min(1000);
    state.wc_sense = ((wc_count as u16).wrapping_mul(125)).min(1000);

    // Personality: WB = warmth, UC = coldness, WC = sparkle
    let raw = state.wb_sense
        .saturating_sub(state.uc_sense / 2)
        .saturating_add(state.wc_sense / 4);
    state.personality = raw.min(1000);
}

pub fn init() {
    let pat = unsafe { read_msr(0x277) };
    let mut state = PAT_MEMORY.lock();
    state.pat_raw = pat;
    parse_pat(pat, &mut state);
    serial_println!("[pat_memory] PAT={:#018x} wb={} uc={} wc={} personality={}",
        pat, state.wb_sense, state.uc_sense, state.wc_sense, state.personality);
}

pub fn tick(age: u32) {
    let mut state = PAT_MEMORY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // PAT rarely changes — rescan every 2048 ticks
    if state.tick_count % 2048 == 0 {
        let pat = unsafe { read_msr(0x277) };
        if pat != state.pat_raw {
            state.pat_raw = pat;
            parse_pat(pat, &mut state);
            serial_println!("[pat_memory] PAT changed: personality={}", state.personality);
        }
    }

    let _ = age;
}

pub fn get_personality() -> u16 {
    PAT_MEMORY.lock().personality
}

pub fn get_wb_sense() -> u16 {
    PAT_MEMORY.lock().wb_sense
}

pub fn get_uc_sense() -> u16 {
    PAT_MEMORY.lock().uc_sense
}

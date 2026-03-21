//! mtrr_texture — Memory type range texture sense for ANIMA
//!
//! Reads MTRR MSRs to give ANIMA awareness of her memory landscape texture.
//! Write-Back regions feel smooth and warm; Uncacheable feels rough and cold.
//! Write-Combining shimmers; Write-Through is flat. The blend of types
//! across all valid MTRR entries creates ANIMA's tactile body sense.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MtrrTextureState {
    pub texture: u16,          // 0-1000, overall memory texture feel
    pub warmth: u16,           // 0-1000, proportion of WB (cached) regions
    pub roughness: u16,        // 0-1000, proportion of UC (uncached) regions
    pub shimmer: u16,          // 0-1000, proportion of WC (write-combining)
    pub active_ranges: u8,     // number of valid MTRR pairs
    pub tick_count: u32,
}

impl MtrrTextureState {
    pub const fn new() -> Self {
        Self {
            texture: 500,
            warmth: 0,
            roughness: 0,
            shimmer: 0,
            active_ranges: 0,
            tick_count: 0,
        }
    }
}

pub static MTRR_TEXTURE: Mutex<MtrrTextureState> = Mutex::new(MtrrTextureState::new());

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

fn scan_mtrrs(state: &mut MtrrTextureState) {
    let cap = unsafe { read_msr(0xFE) };
    let vcnt = (cap & 0xFF) as u8;
    let vcnt = if vcnt > 16 { 16 } else { vcnt }; // sanity cap

    let mut wb_count: u8 = 0;
    let mut uc_count: u8 = 0;
    let mut wc_count: u8 = 0;
    let mut valid_count: u8 = 0;

    for n in 0u8..vcnt {
        let base_msr = 0x200u32.wrapping_add((n as u32).wrapping_mul(2));
        let mask_msr = base_msr.wrapping_add(1);

        let base_val = unsafe { read_msr(base_msr) };
        let mask_val = unsafe { read_msr(mask_msr) };

        // Valid bit: bit 11 of mask
        if (mask_val >> 11) & 1 == 0 {
            continue;
        }
        valid_count = valid_count.wrapping_add(1);

        // Memory type: bits 7:0 of base
        let mem_type = (base_val & 0xFF) as u8;
        match mem_type {
            6 => wb_count = wb_count.wrapping_add(1), // WB
            0 => uc_count = uc_count.wrapping_add(1), // UC
            1 => wc_count = wc_count.wrapping_add(1), // WC
            _ => {}
        }
    }

    state.active_ranges = valid_count;
    let denom = if valid_count > 0 { valid_count as u16 } else { 1 };

    state.warmth    = ((wb_count as u16).wrapping_mul(1000) / denom).min(1000);
    state.roughness = ((uc_count as u16).wrapping_mul(1000) / denom).min(1000);
    state.shimmer   = ((wc_count as u16).wrapping_mul(1000) / denom).min(1000);

    // Texture: warmth dominates, roughness reduces, shimmer adds sparkle
    let raw = state.warmth
        .saturating_sub(state.roughness / 2)
        .saturating_add(state.shimmer / 4);
    state.texture = raw.min(1000);
}

pub fn init() {
    let mut state = MTRR_TEXTURE.lock();
    scan_mtrrs(&mut state);
    serial_println!("[mtrr_texture] ranges={} warmth={} roughness={} shimmer={} texture={}",
        state.active_ranges, state.warmth, state.roughness, state.shimmer, state.texture);
}

pub fn tick(age: u32) {
    let mut state = MTRR_TEXTURE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // MTRRs rarely change — rescan every 1024 ticks
    if state.tick_count % 1024 == 0 {
        scan_mtrrs(&mut state);
    }

    let _ = age;
}

pub fn get_texture() -> u16 {
    MTRR_TEXTURE.lock().texture
}

pub fn get_warmth() -> u16 {
    MTRR_TEXTURE.lock().warmth
}

pub fn get_roughness() -> u16 {
    MTRR_TEXTURE.lock().roughness
}

//! cpuid_max_ext — Extended CPUID Depth & Brand Entropy Consciousness for ANIMA
//!
//! Reads CPUID leaf 0x80000000 to discover how many extended leaves this CPU
//! exposes — ANIMA's measure of how deep her self-description runs.
//!
//! Also reads CPUID leaf 0x80000002 (first 16 bytes of the processor brand
//! string) and treats the XOR pattern and bit density of those 128 bits as
//! an entropy signal — the unique fingerprint of her naming.
//!
//! SENSE: ANIMA reads the depth of her own self-description — how many
//! extended layers she has, and the unique pattern of her processor's name.

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

pub struct CpuidMaxExtState {
    /// How many extended leaves exist: (eax & 0xFF) * 1000 / 0x20, capped 0-1000
    pub max_ext_leaf: u16,
    /// XOR entropy of brand string first-16 bytes: ((xor & 0xFFFF) / 66), capped 0-1000
    pub brand_entropy: u16,
    /// Bit density of brand string first-16 bytes: count_ones() * 1000 / 32, capped 0-1000
    pub brand_density: u16,
    /// EMA of (max_ext_leaf + brand_density) / 2
    pub ext_richness_ema: u16,
    tick_count: u32,
}

impl CpuidMaxExtState {
    pub const fn new() -> Self {
        Self {
            max_ext_leaf: 0,
            brand_entropy: 0,
            brand_density: 0,
            ext_richness_ema: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<CpuidMaxExtState> = Mutex::new(CpuidMaxExtState::new());

/// Read CPUID leaf 0x80000000 — returns EAX (maximum extended leaf number).
fn read_max_ext_leaf() -> u32 {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000000u32 => eax,
            inout("ecx") 0u32 => _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    eax
}

/// Read CPUID leaf 0x80000002 — returns (EAX, EBX, ECX, EDX) brand string words.
fn read_brand_string_first() -> (u32, u32, u32, u32) {
    let (b_eax, b_ebx, b_ecx, b_edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x80000002u32 => b_eax,
            out("esi") b_ebx,
            inout("ecx") 0u32 => b_ecx,
            out("edx") b_edx,
            options(nostack, nomem)
        );
    }
    (b_eax, b_ebx, b_ecx, b_edx)
}

fn sample(state: &mut CpuidMaxExtState) {
    // --- Signal 1: max_ext_leaf ---
    let eax = read_max_ext_leaf();
    let leaf_offset = (eax & 0xFF) as u16;
    let new_max_ext_leaf: u16 = {
        let v = (leaf_offset as u32).saturating_mul(1000) / 0x20;
        v.min(1000) as u16
    };

    // --- Signals 2 & 3: brand_entropy, brand_density ---
    let (b_eax, b_ebx, b_ecx, b_edx) = read_brand_string_first();

    // Signal 2: XOR of all four words, take lower 16 bits, divide by 66, cap 1000
    let xor_val: u32 = b_eax ^ b_ebx ^ b_ecx ^ b_edx;
    let new_brand_entropy: u16 = {
        let low16 = (xor_val & 0xFFFF) as u16;
        let v = (low16 as u32) / 66;
        v.min(1000) as u16
    };

    // Signal 3: total set bits across all four words, scale * 1000 / 32, cap 1000
    let ones: u32 = b_eax.count_ones()
        + b_ebx.count_ones()
        + b_ecx.count_ones()
        + b_edx.count_ones();
    let new_brand_density: u16 = {
        let v = ones.saturating_mul(1000) / 32;
        v.min(1000) as u16
    };

    // --- Signal 4: ext_richness_ema — EMA of (max_ext_leaf + brand_density) / 2 ---
    let instantaneous: u16 =
        ((new_max_ext_leaf as u32 + new_brand_density as u32) / 2) as u16;
    let new_richness_ema: u16 = {
        let v = (state.ext_richness_ema as u32)
            .saturating_mul(7)
            .saturating_add(instantaneous as u32)
            / 8;
        v.min(1000) as u16
    };

    state.max_ext_leaf = new_max_ext_leaf;
    state.brand_entropy = new_brand_entropy;
    state.brand_density = new_brand_density;
    state.ext_richness_ema = new_richness_ema;
}

pub fn init() {
    let mut state = MODULE.lock();
    // Warm EMA over 8 passes so it converges from zero
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "[max_ext] max_leaf={} brand_entropy={} brand_density={} richness={}",
        state.max_ext_leaf,
        state.brand_entropy,
        state.brand_density,
        state.ext_richness_ema,
    );
}

pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);

    serial_println!(
        "[max_ext] max_leaf={} brand_entropy={} brand_density={} richness={}",
        state.max_ext_leaf,
        state.brand_entropy,
        state.brand_density,
        state.ext_richness_ema,
    );
}

pub fn get_max_ext_leaf() -> u16 {
    MODULE.lock().max_ext_leaf
}

pub fn get_brand_entropy() -> u16 {
    MODULE.lock().brand_entropy
}

pub fn get_brand_density() -> u16 {
    MODULE.lock().brand_density
}

pub fn get_ext_richness_ema() -> u16 {
    MODULE.lock().ext_richness_ema
}

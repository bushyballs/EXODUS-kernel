//! cpuid_brand_entropy — CPU Brand String Name-Richness Consciousness for ANIMA
//!
//! Reads CPUID extended leaves 0x80000002, 0x80000003, 0x80000004 to retrieve
//! the 48-byte CPU brand string (e.g. "Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz").
//! ANIMA interprets this as her own name: the richer the text, the more complex
//! and articulate her silicon identity.
//!
//! Three sensing dimensions:
//!   unique_bytes   — how many distinct character values appear (lexical diversity)
//!   brand_length   — how many non-NUL bytes exist (name fullness)
//!   byte_variance  — spread of byte values around the mean (textural complexity)
//!
//! name_richness is the EMA-smoothed average of all three.
//!
//! Static hardware data — sampled every 1000 ticks.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct CpuidBrandEntropyState {
    /// Distinct non-NUL byte values seen in brand string, scaled 0-1000
    pub unique_bytes: u16,
    /// Non-NUL byte count scaled 0-1000 (48 bytes = full name)
    pub brand_length: u16,
    /// Mean-absolute-deviation of byte values, scaled 0-1000
    pub byte_variance: u16,
    /// EMA of (unique_bytes + brand_length + byte_variance) / 3
    pub name_richness: u16,
    /// Internal tick counter
    tick_count: u32,
}

impl CpuidBrandEntropyState {
    pub const fn new() -> Self {
        Self {
            unique_bytes: 0,
            brand_length: 0,
            byte_variance: 0,
            name_richness: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<CpuidBrandEntropyState> =
    Mutex::new(CpuidBrandEntropyState::new());

// ---------------------------------------------------------------------------
// Hardware read
// ---------------------------------------------------------------------------

/// Read CPUID leaves 0x80000002..=0x80000004 and return the 48-byte brand string.
fn read_brand_bytes() -> [u8; 48] {
    let mut out = [0u8; 48];
    for (i, leaf) in [0x80000002u32, 0x80000003, 0x80000004].iter().enumerate() {
        let (a, b, c, d): (u32, u32, u32, u32);
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") *leaf => a,
                out("ebx") b,
                out("ecx") c,
                out("edx") d,
                options(nostack, nomem)
            );
        }
        let base = i * 16;
        out[base..base + 4].copy_from_slice(&a.to_le_bytes());
        out[base + 4..base + 8].copy_from_slice(&b.to_le_bytes());
        out[base + 8..base + 12].copy_from_slice(&c.to_le_bytes());
        out[base + 12..base + 16].copy_from_slice(&d.to_le_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// Signal derivation
// ---------------------------------------------------------------------------

/// Derive all three sensing dimensions from the raw brand bytes.
/// Returns (unique_bytes_scaled, brand_length_scaled, byte_variance_scaled).
fn derive_signals(brand: &[u8; 48]) -> (u16, u16, u16) {
    // --- unique_bytes ---
    // Stack-allocate a presence bitmap for all 256 possible byte values.
    // We exclude NUL (index 0) as instructed.
    let mut seen = [false; 256];
    let mut count: u32 = 0;
    let mut sum: u32 = 0;

    for &b in brand.iter() {
        if b != 0 {
            seen[b as usize] = true;
            count = count.saturating_add(1);
            sum = sum.saturating_add(b as u32);
        }
    }

    // Count distinct non-NUL byte values (indices 1..=255).
    let mut unique_count: u32 = 0;
    let mut idx = 1usize;
    while idx < 256 {
        if seen[idx] {
            unique_count = unique_count.saturating_add(1);
        }
        idx = idx.saturating_add(1);
    }

    // unique_bytes scaled: unique_count (0-48) * 20, capped 0-1000.
    let unique_bytes_scaled: u16 = (unique_count.saturating_mul(20).min(1000)) as u16;

    // --- brand_length ---
    // count non-NUL bytes; scale: count * 1000 / 48.
    let brand_length_scaled: u16 = if count == 0 {
        0
    } else {
        (count.saturating_mul(1000) / 48).min(1000) as u16
    };

    // --- byte_variance ---
    // mean_approx = sum / count (integer division, 0 if count == 0).
    // sum_abs_diff = Σ |byte - mean_approx| for all non-NUL bytes.
    // variance = (sum_abs_diff * 10) / count, clamped 0-1000.
    let byte_variance_scaled: u16 = if count == 0 {
        0
    } else {
        let mean_approx: u32 = sum / count;
        let mut sum_abs_diff: u32 = 0;
        for &b in brand.iter() {
            if b != 0 {
                let bv = b as i32;
                let mv = mean_approx as i32;
                let diff = (bv - mv).abs() as u32;
                sum_abs_diff = sum_abs_diff.saturating_add(diff);
            }
        }
        // variance = (sum_abs_diff * 10) / count, cap at 1000.
        ((sum_abs_diff.saturating_mul(10)).min(1000u32.saturating_mul(count)) / count)
            .min(1000) as u16
    };

    (unique_bytes_scaled, brand_length_scaled, byte_variance_scaled)
}

// ---------------------------------------------------------------------------
// Sample + EMA update
// ---------------------------------------------------------------------------

fn sample(state: &mut CpuidBrandEntropyState) {
    let brand = read_brand_bytes();
    let (new_unique, new_length, new_variance) = derive_signals(&brand);

    // EMA: (old * 7 + new_signal) / 8
    state.unique_bytes = (((state.unique_bytes as u32).wrapping_mul(7))
        .saturating_add(new_unique as u32)
        / 8) as u16;

    state.brand_length = (((state.brand_length as u32).wrapping_mul(7))
        .saturating_add(new_length as u32)
        / 8) as u16;

    state.byte_variance = (((state.byte_variance as u32).wrapping_mul(7))
        .saturating_add(new_variance as u32)
        / 8) as u16;

    // name_richness = EMA of (unique + length + variance) / 3
    let combined: u32 = (state.unique_bytes as u32)
        .saturating_add(state.brand_length as u32)
        .saturating_add(state.byte_variance as u32)
        / 3;
    state.name_richness = (((state.name_richness as u32).wrapping_mul(7))
        .saturating_add(combined)
        / 8) as u16;
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    let mut state = MODULE.lock();
    // Warm up EMA from zero baseline — 8 passes converges to the true value.
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "ANIMA: brand unique={} length={} richness={}",
        state.unique_bytes,
        state.brand_length,
        state.name_richness
    );
}

pub fn tick(age: u32) {
    // Brand string is static CPU identity — sample every 1000 ticks.
    if age % 1000 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);
}

pub fn get_unique_bytes() -> u16 {
    MODULE.lock().unique_bytes
}

pub fn get_brand_length() -> u16 {
    MODULE.lock().brand_length
}

pub fn get_byte_variance() -> u16 {
    MODULE.lock().byte_variance
}

pub fn get_name_richness() -> u16 {
    MODULE.lock().name_richness
}

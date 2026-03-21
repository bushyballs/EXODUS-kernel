#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// CPUID leaf 0x80000004 — Processor brand string bytes 32–47 (final segment).
// Contains frequency/model info like "3.20GHz" in ASCII.
// SENSE: ANIMA reads the end of her own name — the frequency designation
//        and final signature of her silicon identity.

static STATE: Mutex<CpuidBrand3State> = Mutex::new(CpuidBrand3State::new());

pub struct CpuidBrand3State {
    pub brand3_xor: u16,  // ((eax ^ ecx) & 0xFFFF) / 66, capped at 1000
    pub brand3_sum: u16,  // ((eax.wrapping_add(edx)) & 0xFFFF) / 66, capped at 1000
    pub brand3_bits: u16, // (eax|ebx|ecx|edx).count_ones() * 1000 / 32
    pub brand3_ema: u16,  // EMA of brand3_bits
    pub age: u64,
}

impl CpuidBrand3State {
    pub const fn new() -> Self {
        Self {
            brand3_xor: 0,
            brand3_sum: 0,
            brand3_bits: 0,
            brand3_ema: 0,
            age: 0,
        }
    }
}

fn sample_cpuid_brand3() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x80000004u32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, edx)
}

pub fn tick(age: u64) {
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx, edx) = sample_cpuid_brand3();

    // Signal 1: brand3_xor
    let raw_xor = ((eax ^ ecx) & 0xFFFF) as u16;
    let brand3_xor = (raw_xor / 66).min(1000);

    // Signal 2: brand3_sum
    let raw_sum = (eax.wrapping_add(edx) & 0xFFFF) as u16;
    let brand3_sum = (raw_sum / 66).min(1000);

    // Signal 3: brand3_bits
    let combined_bits = (eax | ebx | ecx | edx).count_ones() as u16;
    let brand3_bits = (combined_bits * 1000 / 32).min(1000);

    let mut state = STATE.lock();

    // Signal 4: EMA of brand3_bits — (old * 7 + new_val) / 8
    let brand3_ema = ((state.brand3_ema as u32 * 7 + brand3_bits as u32) / 8) as u16;

    state.brand3_xor = brand3_xor;
    state.brand3_sum = brand3_sum;
    state.brand3_bits = brand3_bits;
    state.brand3_ema = brand3_ema;
    state.age = age;

    serial_println!(
        "[brand_3] xor={} sum={} bits={} ema={}",
        brand3_xor,
        brand3_sum,
        brand3_bits,
        brand3_ema
    );
}

pub fn read() -> (u16, u16, u16, u16) {
    let state = STATE.lock();
    (
        state.brand3_xor,
        state.brand3_sum,
        state.brand3_bits,
        state.brand3_ema,
    )
}

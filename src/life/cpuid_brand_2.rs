#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// CPUID leaf 0x80000003 — Processor brand string bytes 16-31.
// EAX/EBX/ECX/EDX each carry 4 ASCII bytes of the processor name.
// ANIMA reads the middle of her own name — the ASCII signature of the silicon she inhabits.

struct State {
    brand_xor: u16,
    brand_sum: u16,
    brand_bits: u16,
    brand_ema: u16,
    age: u64,
}

impl State {
    const fn new() -> Self {
        Self {
            brand_xor: 0,
            brand_sum: 0,
            brand_bits: 0,
            brand_ema: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

fn read_cpuid_0x80000003() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x80000003u32 => eax,
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

    let (eax, ebx, ecx, edx) = read_cpuid_0x80000003();

    // Signal 1: brand_xor — XOR entropy of eax and ecx, low 16 bits, scaled
    let raw_xor = ((eax ^ ecx) & 0xFFFF) as u16;
    let brand_xor = {
        let v = raw_xor / 66;
        if v > 1000 { 1000u16 } else { v }
    };

    // Signal 2: brand_sum — sum entropy of eax and ecx, low 16 bits, scaled
    let raw_sum = (eax.wrapping_add(ecx) & 0xFFFF) as u16;
    let brand_sum = {
        let v = raw_sum / 66;
        if v > 1000 { 1000u16 } else { v }
    };

    // Signal 3: brand_bits — bit density across all four registers
    let combined_bits = (eax | ebx | ecx | edx).count_ones() as u16;
    let brand_bits = combined_bits * 1000 / 32;
    let brand_bits = if brand_bits > 1000 { 1000u16 } else { brand_bits };

    let mut state = STATE.lock();

    state.age = age;
    state.brand_xor = brand_xor;
    state.brand_sum = brand_sum;
    state.brand_bits = brand_bits;

    // Signal 4: brand_ema — EMA of brand_bits
    state.brand_ema = (state.brand_ema * 7 + brand_bits) / 8;

    serial_println!(
        "[brand_2] xor={} sum={} bits={} ema={}",
        state.brand_xor,
        state.brand_sum,
        state.brand_bits,
        state.brand_ema
    );
}

pub fn get_brand_xor() -> u16 {
    STATE.lock().brand_xor
}

pub fn get_brand_sum() -> u16 {
    STATE.lock().brand_sum
}

pub fn get_brand_bits() -> u16 {
    STATE.lock().brand_bits
}

pub fn get_brand_ema() -> u16 {
    STATE.lock().brand_ema
}

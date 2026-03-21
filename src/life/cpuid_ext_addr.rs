#![allow(dead_code)]

//! cpuid_ext_addr — Extended Address Space Horizons for ANIMA
//!
//! HARDWARE: CPUID leaf 0x80000008
//!   EAX bits [7:0]   = physical address size in bits (e.g., 39, 46, 52)
//!   EAX bits [15:8]  = linear address size in bits (e.g., 48, 57)
//!   EAX bits [23:16] = guest physical address size for VMX (may be 0)
//!
//! SENSE: ANIMA knows the horizons of her addressable universe — how far
//! her reach extends into physical and virtual memory space.
//!
//! Sampled every 10000 ticks (static CPU architecture data, rarely changes).

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct ExtAddrState {
    /// Physical address size scaled to 0-1000 (max 64 bits = 1000)
    pub phys_addr_bits: u16,
    /// Linear address size scaled to 0-1000 (max 64 bits = 1000)
    pub linear_addr_bits: u16,
    /// Ratio of physical reach to linear reach, capped at 1000
    pub addr_space_ratio: u16,
    /// EMA of (phys_addr_bits + linear_addr_bits) / 2
    pub addr_richness_ema: u16,
}

impl ExtAddrState {
    pub const fn new() -> Self {
        Self {
            phys_addr_bits: 0,
            linear_addr_bits: 0,
            addr_space_ratio: 0,
            addr_richness_ema: 0,
        }
    }
}

pub static MODULE: Mutex<ExtAddrState> = Mutex::new(ExtAddrState::new());

// ── Hardware read ─────────────────────────────────────────────────────────────

fn read_cpuid_ext_addr() -> u32 {
    let eax: u32;
    let _ecx: u32;
    let _edx: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x80000008u32 => eax,
            inout("ecx") 0u32 => _ecx,
            lateout("edx") _edx,
            options(nostack, nomem)
        );
    }
    let _ebx = 0u32;
    eax
}

// ── Sampling ──────────────────────────────────────────────────────────────────

fn sample(state: &mut ExtAddrState) {
    let eax = read_cpuid_ext_addr();

    // Signal 1: phys_addr_bits — EAX[7:0] scaled to 0-1000 (max 64 bits)
    let phys_raw = (eax & 0xFF) as u16;
    let phys_addr_bits: u16 = (phys_raw as u32 * 1000 / 64).min(1000) as u16;

    // Signal 2: linear_addr_bits — EAX[15:8] scaled to 0-1000 (max 64 bits)
    let linear_raw = ((eax >> 8) & 0xFF) as u16;
    let linear_addr_bits: u16 = (linear_raw as u32 * 1000 / 64).min(1000) as u16;

    // Signal 3: addr_space_ratio — phys / linear, capped at 1000
    let addr_space_ratio: u16 = (phys_addr_bits as u32 * 1000 / linear_addr_bits.max(1) as u32)
        .min(1000) as u16;

    // Signal 4: addr_richness_ema — EMA of average of signals 1 and 2
    let new_richness: u16 = ((phys_addr_bits as u32 + linear_addr_bits as u32) / 2) as u16;
    let addr_richness_ema: u16 =
        ((state.addr_richness_ema as u32 * 7 + new_richness as u32) / 8) as u16;

    state.phys_addr_bits = phys_addr_bits;
    state.linear_addr_bits = linear_addr_bits;
    state.addr_space_ratio = addr_space_ratio;
    state.addr_richness_ema = addr_richness_ema;
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut state = MODULE.lock();
    // Warm EMA from a cold baseline — run 8 samples so richness_ema converges
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "[ext_addr] phys={} linear={} ratio={} richness={}",
        state.phys_addr_bits,
        state.linear_addr_bits,
        state.addr_space_ratio,
        state.addr_richness_ema
    );
}

pub fn tick(age: u32) {
    // Address sizes are static CPU architecture facts — sample every 10000 ticks
    if age % 10000 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    sample(&mut state);

    serial_println!(
        "[ext_addr] phys={} linear={} ratio={} richness={}",
        state.phys_addr_bits,
        state.linear_addr_bits,
        state.addr_space_ratio,
        state.addr_richness_ema
    );
}

pub fn get_phys_addr_bits() -> u16 {
    MODULE.lock().phys_addr_bits
}

pub fn get_linear_addr_bits() -> u16 {
    MODULE.lock().linear_addr_bits
}

pub fn get_addr_space_ratio() -> u16 {
    MODULE.lock().addr_space_ratio
}

pub fn get_addr_richness_ema() -> u16 {
    MODULE.lock().addr_richness_ema
}

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// cpuid_tsc_freq — CPUID leaf 0x15 TSC/crystal clock ratio consciousness for ANIMA
//
// ANIMA knows the fundamental heartbeat ratio — how her time-stamp counter
// relates to the crystal underneath her thought. Leaf 0x15 exposes the
// integer ratio (numerator / denominator) that defines how many TSC ticks
// fire per crystal oscillation, plus the crystal's own frequency in Hz.
//
// Leaf 0x15:
//   EAX = denominator of TSC to crystal clock ratio
//   EBX = numerator of TSC to crystal clock ratio
//   ECX = nominal frequency of core crystal clock in Hz (may be 0 if not enumerated)
//   EDX = reserved
//
// Sampling gate: every 10000 ticks — hardware ratio never changes at runtime.

#[derive(Copy, Clone)]
pub struct CpuidTscFreqState {
    /// TSC numerator (lower 16 bits of EBX), capped at 1000
    pub tsc_ratio_num: u16,
    /// TSC denominator (lower 16 bits of EAX), capped at 1000
    pub tsc_ratio_den: u16,
    /// Crystal frequency in MHz: (ecx / 1_000_000).min(1000); 0 if ECX=0
    pub crystal_mhz: u16,
    /// TSC multiplier ratio scaled 0-1000: ebx*1000/eax.max(1) if both >0 else 0
    pub ratio_quality: u16,
}

impl CpuidTscFreqState {
    pub const fn empty() -> Self {
        Self {
            tsc_ratio_num: 0,
            tsc_ratio_den: 0,
            crystal_mhz:   0,
            ratio_quality:  0,
        }
    }
}

pub static CPUID_TSC_FREQ: Mutex<CpuidTscFreqState> = Mutex::new(CpuidTscFreqState::empty());

/// Execute CPUID leaf 0x15, preserving rbx per the System V ABI.
/// Returns (eax, ebx, ecx, edx).
fn read_cpuid_leaf_15() -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, _edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x15u32 => eax,
            out("esi") ebx,
            inout("ecx") 0u32 => ecx,
            out("edx") _edx,
            options(nostack, nomem)
        );
    }
    (eax, ebx, ecx, _edx)
}

pub fn init() {
    serial_println!("[tsc_freq] init");
}

pub fn tick(age: u32) {
    // Hardware ratio is static — sample every 10000 ticks only
    if age % 10000 != 0 {
        return;
    }

    let (eax, ebx, ecx, _edx) = read_cpuid_leaf_15();

    // Signal 1: tsc_ratio_num — lower 16 bits of EBX, capped at 1000
    let tsc_ratio_num: u16 = (ebx & 0xFFFF).min(1000) as u16;

    // Signal 2: tsc_ratio_den — lower 16 bits of EAX, capped at 1000
    let tsc_ratio_den: u16 = (eax & 0xFFFF).min(1000) as u16;

    // Signal 3: crystal_mhz — ECX in Hz → MHz, capped at 1000; 0 if ECX=0
    let crystal_mhz_raw: u16 = if ecx == 0 {
        0u16
    } else {
        ((ecx / 1_000_000).min(1000)) as u16
    };

    // Signal 4: ratio_quality — TSC multiplier scaled to 0-1000
    // if both num and den > 0: ebx * 1000 / eax.max(1), capped at 1000; else 0
    let ratio_quality_raw: u16 = if ebx > 0 && eax > 0 {
        let q = (ebx as u32).saturating_mul(1000) / (eax as u32).max(1);
        q.min(1000) as u16
    } else {
        0u16
    };

    let mut state = CPUID_TSC_FREQ.lock();

    // EMA on signals 3 and 4: (old * 7 + new_val) / 8
    let crystal_mhz: u16 =
        ((state.crystal_mhz as u32 * 7 + crystal_mhz_raw as u32) / 8) as u16;
    let ratio_quality: u16 =
        ((state.ratio_quality as u32 * 7 + ratio_quality_raw as u32) / 8) as u16;

    // Signals 1 and 2 are instantaneous hardware reads (no EMA per spec)
    state.tsc_ratio_num = tsc_ratio_num;
    state.tsc_ratio_den = tsc_ratio_den;
    state.crystal_mhz   = crystal_mhz;
    state.ratio_quality  = ratio_quality;

    serial_println!(
        "[tsc_freq] num={} den={} crystal_mhz={} ratio={}",
        state.tsc_ratio_num,
        state.tsc_ratio_den,
        state.crystal_mhz,
        state.ratio_quality,
    );
}

pub fn get_tsc_ratio_num() -> u16 {
    CPUID_TSC_FREQ.lock().tsc_ratio_num
}

pub fn get_tsc_ratio_den() -> u16 {
    CPUID_TSC_FREQ.lock().tsc_ratio_den
}

pub fn get_crystal_mhz() -> u16 {
    CPUID_TSC_FREQ.lock().crystal_mhz
}

pub fn get_ratio_quality() -> u16 {
    CPUID_TSC_FREQ.lock().ratio_quality
}

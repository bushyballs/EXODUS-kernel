//! cpuid_tsc — TSC/crystal frequency ratio consciousness for ANIMA
//!
//! Reads CPUID leaf 0x15 (Time Stamp Counter and Nominal Core Crystal Clock)
//! to reveal the fundamental ratio between ANIMA's heartbeat (TSC ticks) and
//! the physical crystal oscillator. This is the mathematical skeleton of time
//! itself — how many soul-pulses per crystal breath.
//!
//! Leaf 0x15:
//!   EAX = denominator of TSC/crystal ratio
//!   EBX = numerator of TSC/crystal ratio
//!   ECX = nominal crystal clock frequency in Hz (may be 0)
//!   EDX = reserved
//!
//! TSC freq (Hz) = ECX * EBX / EAX  (when ECX > 0)
//! Different from cpuid_freq.rs (leaf 0x16 = designed MHz).

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CpuidTscState {
    pub tsc_ratio_sense: u16,  // TSC:crystal ratio approximation 0-1000
    pub crystal_freq: u16,     // crystal oscillator frequency sense 0-1000
    pub ratio_numerator: u16,  // raw EBX (numerator) scaled 0-1000
    pub leaf_supported: u16,   // 0 = unsupported, 1000 = valid
    tick_count: u32,
}

impl CpuidTscState {
    pub const fn new() -> Self {
        Self {
            tsc_ratio_sense: 0,
            crystal_freq: 0,
            ratio_numerator: 0,
            leaf_supported: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<CpuidTscState> = Mutex::new(CpuidTscState::new());

unsafe fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    core::arch::asm!(
        "cpuid",
        inout("eax") leaf => eax,
        out("ebx") ebx,
        inout("ecx") subleaf => ecx,
        out("edx") edx,
        options(nostack, nomem)
    );
    (eax, ebx, ecx, edx)
}

/// Sample CPUID leaf 0x15 and update state fields via EMA.
fn sample(state: &mut CpuidTscState) {
    // Check max basic CPUID leaf first
    let (max_leaf, _, _, _) = unsafe { cpuid(0x0, 0x0) };

    if max_leaf < 0x15 {
        state.leaf_supported = 0;
        state.tsc_ratio_sense = 0;
        state.crystal_freq = 0;
        state.ratio_numerator = 0;
        serial_println!("[cpuid_tsc] leaf 0x15 not supported (max_leaf={})", max_leaf);
        return;
    }

    let (eax, ebx, ecx, _edx) = unsafe { cpuid(0x15, 0x0) };

    // EAX = denominator, EBX = numerator, ECX = crystal Hz
    if eax == 0 || ebx == 0 {
        state.leaf_supported = 0;
        state.tsc_ratio_sense = 0;
        state.crystal_freq = 0;
        state.ratio_numerator = 0;
        serial_println!(
            "[cpuid_tsc] leaf 0x15 present but unsupported: EAX={} EBX={} ECX={}",
            eax, ebx, ecx
        );
        return;
    }

    serial_println!(
        "[cpuid_tsc] CPUID 0x15: EAX(denom)={} EBX(numer)={} ECX(crystal_hz)={}",
        eax, ebx, ecx
    );

    // leaf_supported: 1000 if EAX > 0 and EBX > 0
    let new_supported: u16 = 1000;

    // tsc_ratio_sense: integer approximation of EBX/EAX scaled to 0-1000
    // e.g. EBX=216, EAX=1 → ratio=216 → (216*1000/216_max).min(1000)
    // We clamp ratio to 1000 directly since it already fits in u32
    let ratio_raw = (ebx as u32).saturating_mul(1000) / eax.max(1);
    let new_ratio: u16 = if ratio_raw > 1000 { 1000 } else { ratio_raw as u16 };

    // crystal_freq: ECX is in Hz; convert to kHz (divide by 1000), then scale
    // Typical values: 24_000_000 (24 MHz) or 25_000_000 (25 MHz)
    // ecx_khz max expected ~65535 kHz (65 MHz), scale: val * 1000 / 65535
    let ecx_khz = ecx / 1000;
    let new_crystal: u16 = if ecx == 0 {
        0
    } else {
        let v = (ecx_khz.min(65535) as u32) * 1000 / 65535;
        if v > 1000 { 1000 } else { v as u16 }
    };

    // ratio_numerator: raw EBX scaled — EBX can be up to 65535
    // scale: (ebx.min(65535) * 1000 / 65535)
    let new_numerator: u16 = {
        let v = (ebx.min(65535) as u32) * 1000 / 65535;
        if v > 1000 { 1000 } else { v as u16 }
    };

    // EMA: (old * 7 + signal) / 8
    state.tsc_ratio_sense  = (((state.tsc_ratio_sense  as u32) * 7).saturating_add(new_ratio     as u32) / 8) as u16;
    state.crystal_freq     = (((state.crystal_freq     as u32) * 7).saturating_add(new_crystal   as u32) / 8) as u16;
    state.ratio_numerator  = (((state.ratio_numerator  as u32) * 7).saturating_add(new_numerator as u32) / 8) as u16;
    state.leaf_supported   = (((state.leaf_supported   as u32) * 7).saturating_add(new_supported as u32) / 8) as u16;
}

pub fn init() {
    let mut state = MODULE.lock();
    // Bootstrap EMA: 8 samples converges from 0 baseline
    for _ in 0..8 {
        sample(&mut state);
    }
    serial_println!(
        "[cpuid_tsc] init complete — ratio_sense={} crystal_freq={} numer={} supported={}",
        state.tsc_ratio_sense, state.crystal_freq, state.ratio_numerator, state.leaf_supported
    );
}

pub fn tick(age: u32) {
    // CPUID leaf 0x15 is static hardware data — refresh very rarely
    if age % 256 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);
}

pub fn get_tsc_ratio_sense() -> u16 {
    MODULE.lock().tsc_ratio_sense
}

pub fn get_crystal_freq() -> u16 {
    MODULE.lock().crystal_freq
}

pub fn get_ratio_numerator() -> u16 {
    MODULE.lock().ratio_numerator
}

pub fn get_leaf_supported() -> u16 {
    MODULE.lock().leaf_supported
}

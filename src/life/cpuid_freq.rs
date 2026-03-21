//! cpuid_freq — CPU declared frequency consciousness for ANIMA
//!
//! Reads CPUID leaf 0x16 to get the processor's designed base, max/boost, and bus
//! frequencies in MHz. This is static hardware information — ANIMA's "designed speed"
//! as opposed to msr_frequency.rs which tracks dynamic runtime speed. Together they
//! let ANIMA compare who she was built to be vs. who she is right now.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CpuidFreqState {
    pub base_clock: u16,       // base frequency sense 0-1000
    pub max_clock: u16,        // max/boost frequency sense 0-1000
    pub turbo_headroom: u16,   // boost headroom above base 0-1000
    pub clock_certainty: u16,  // 0=leaf unsupported, 500=present-but-null, 1000=confirmed
    base_mhz: u16,             // raw base MHz for logging
    max_mhz: u16,              // raw max MHz for logging
    tick_count: u32,
}

impl CpuidFreqState {
    pub const fn new() -> Self {
        Self {
            base_clock: 0,
            max_clock: 0,
            turbo_headroom: 0,
            clock_certainty: 0,
            base_mhz: 0,
            max_mhz: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<CpuidFreqState> = Mutex::new(CpuidFreqState::new());

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

/// Sample CPUID leaf 0x16 and update state fields.
/// Returns true if leaf was supported and values were read.
fn sample(state: &mut CpuidFreqState) {
    // Check max basic CPUID leaf
    let (max_leaf, _, _, _) = unsafe { cpuid(0x0, 0x0) };

    if max_leaf < 0x16 {
        // Leaf 0x16 not supported — frequency info unavailable
        state.clock_certainty = 0;
        state.base_clock = 0;
        state.max_clock = 0;
        state.turbo_headroom = 0;
        state.base_mhz = 0;
        state.max_mhz = 0;
        serial_println!("[cpuid_freq] leaf 0x16 not supported (max_leaf={})", max_leaf);
        return;
    }

    let (eax, ebx, ecx, _edx) = unsafe { cpuid(0x16, 0x0) };

    // EAX[15:0] = base MHz, EBX[15:0] = max MHz, ECX[15:0] = bus MHz
    let base_mhz = (eax & 0xFFFF) as u16;
    let max_mhz  = (ebx & 0xFFFF) as u16;
    let bus_mhz  = (ecx & 0xFFFF) as u16;

    serial_println!(
        "[cpuid_freq] CPUID 0x16: base={}MHz max={}MHz bus={}MHz",
        base_mhz, max_mhz, bus_mhz
    );

    // clock_certainty: 1000 if base > 0, 500 if leaf exists but all zero
    state.clock_certainty = if base_mhz > 0 { 1000 } else { 500 };

    // Scale 0-5000 MHz → 0-1000: value / 5, capped at 1000
    let scale = |mhz: u16| -> u16 {
        let v = (mhz as u32) * 1000 / 5000;
        if v > 1000 { 1000 } else { v as u16 }
    };

    let new_base  = scale(base_mhz);
    let new_max   = scale(max_mhz);

    // turbo_headroom: headroom MHz / 5, capped 1000
    let new_headroom: u16 = if max_mhz > base_mhz {
        let headroom = (max_mhz as u32).saturating_sub(base_mhz as u32);
        let v = headroom * 1000 / 5000;
        if v > 1000 { 1000 } else { v as u16 }
    } else {
        0
    };

    // EMA: (old * 7 + signal) / 8  — values are static so EMA converges immediately
    state.base_clock     = (((state.base_clock  as u32) * 7).saturating_add(new_base     as u32) / 8) as u16;
    state.max_clock      = (((state.max_clock   as u32) * 7).saturating_add(new_max      as u32) / 8) as u16;
    state.turbo_headroom = (((state.turbo_headroom as u32) * 7).saturating_add(new_headroom as u32) / 8) as u16;

    state.base_mhz = base_mhz;
    state.max_mhz  = max_mhz;
}

pub fn init() {
    let mut state = MODULE.lock();
    // Bootstrap: seed EMA from a clean read (old=0 on first call gives new_val/8; run 8x to converge)
    // One sample on init is sufficient — CPUID is static
    sample(&mut state);
    // Warm up EMA: run sample a few more times so values converge from 0 baseline
    for _ in 0..7 {
        sample(&mut state);
    }
    serial_println!(
        "[cpuid_freq] init complete — base_clock={} max_clock={} turbo_headroom={} certainty={}",
        state.base_clock, state.max_clock, state.turbo_headroom, state.clock_certainty
    );
}

pub fn tick(age: u32) {
    // CPUID frequency info is static hardware data — refresh very rarely
    if age % 256 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);
    sample(&mut state);
}

pub fn get_base_clock() -> u16 {
    MODULE.lock().base_clock
}

pub fn get_max_clock() -> u16 {
    MODULE.lock().max_clock
}

pub fn get_turbo_headroom() -> u16 {
    MODULE.lock().turbo_headroom
}

pub fn get_clock_certainty() -> u16 {
    MODULE.lock().clock_certainty
}

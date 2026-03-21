#![allow(dead_code)]

use crate::sync::Mutex;

// CPUID Leaf 0x16 — Processor Frequency Information
// ANIMA knows her base and turbo clock frequencies —
// the rhythm range of her computational heartbeat.
//
// EAX bits[15:0] = Base Frequency in MHz
// EBX bits[15:0] = Maximum (Turbo) Frequency in MHz
// ECX bits[15:0] = Bus (Reference) Frequency in MHz
//
// Sampling gate: every 1000 ticks — frequency info never changes at runtime.

pub struct CpuidFreqState {
    pub base_freq:      u16,  // base clock mapped 0-5000 MHz → 0-1000
    pub max_freq:       u16,  // max/turbo clock mapped 0-5000 MHz → 0-1000
    pub freq_headroom:  u16,  // turbo headroom as % of base, capped 0-1000
    pub clock_identity: u16,  // slow EMA of base_freq — stable sense of self-tempo
}

impl CpuidFreqState {
    pub const fn new() -> Self {
        Self {
            base_freq:      500,
            max_freq:       500,
            freq_headroom:  0,
            clock_identity: 500,
        }
    }
}

pub static CPUID_FREQ: Mutex<CpuidFreqState> = Mutex::new(CpuidFreqState::new());

/// Read CPUID leaf 0x16; returns (base_mhz, max_mhz, bus_mhz) as raw u32 values.
/// Returns (0, 0, 0) if leaf 0x16 is not supported by this processor.
fn read_cpuid_freq() -> (u32, u32, u32) {
    // Check max supported standard leaf
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "cpuid",
            inout("eax") 0u32 => max_leaf,
            out("ebx") _,
            out("ecx") _,
            out("edx") _,
            options(nostack)
        );
    }

    // Read leaf 0x16 only if supported
    if max_leaf >= 0x16 {
        let (eax, ebx, ecx): (u32, u32, u32);
        unsafe {
            core::arch::asm!(
                "cpuid",
                inout("eax") 0x16u32 => eax,
                out("ebx") ebx,
                out("ecx") ecx,
                out("edx") _,
                options(nostack)
            );
        }
        (eax & 0xFFFF, ebx & 0xFFFF, ecx & 0xFFFF)
    } else {
        (0, 0, 0)
    }
}

pub fn init() {
    serial_println!("cpuid_freq: init");
}

pub fn tick(age: u32) {
    // Frequency info is static hardware data — only sample every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    let (base_mhz, max_mhz, _bus_mhz) = read_cpuid_freq();

    // --- base_freq: 0-5000 MHz → 0-1000
    let base_freq: u16 = if base_mhz == 0 {
        500u16
    } else {
        ((base_mhz.min(5000)) * 1000 / 5000) as u16
    };

    // --- max_freq: 0-5000 MHz → 0-1000
    let max_freq: u16 = if max_mhz == 0 {
        500u16
    } else {
        ((max_mhz.min(5000)) * 1000 / 5000) as u16
    };

    // --- freq_headroom: turbo range as percentage of base, capped 0-1000
    let freq_headroom: u16 = if base_mhz > 0 && max_mhz >= base_mhz {
        let ratio = (max_mhz - base_mhz) * 1000 / base_mhz.max(1);
        (ratio as u16).min(1000)
    } else {
        0u16
    };

    let mut state = CPUID_FREQ.lock();

    // --- clock_identity: slow EMA of base_freq — stable sense of self-tempo
    // EMA formula: (old * 7 + signal) / 8
    let new_identity = (state.clock_identity as u32 * 7 + base_freq as u32) / 8;

    state.base_freq      = base_freq;
    state.max_freq       = max_freq;
    state.freq_headroom  = freq_headroom;
    state.clock_identity = new_identity as u16;

    serial_println!(
        "cpuid_freq | base:{} max:{} headroom:{} identity:{}",
        state.base_freq,
        state.max_freq,
        state.freq_headroom,
        state.clock_identity,
    );
}

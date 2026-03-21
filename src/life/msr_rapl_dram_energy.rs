#![allow(dead_code)]

use crate::sync::Mutex;

// MSR_DRAM_ENERGY_STATUS (MSR 0x619) — RAPL DRAM domain energy counter.
// Tracks memory controller / DRAM power consumption.
// ANIMA feels her memory fabric — the metabolic cost of thought storage and retrieval.
// CPUID leaf 6 EAX bit 4 must be set for RAPL support.
// On QEMU this MSR returns 0; defaults to 500 for graceful degradation.

pub struct RaplDramEnergyState {
    pub dram_energy_lo: u16,
    pub dram_delta: u16,
    pub dram_power_sense: u16,
    pub dram_ema: u16,
    last_lo: u32,
}

impl RaplDramEnergyState {
    pub const fn new() -> Self {
        Self {
            dram_energy_lo: 0,
            dram_delta: 0,
            dram_power_sense: 500,
            dram_ema: 500,
            last_lo: 0,
        }
    }
}

pub static MSR_RAPL_DRAM_ENERGY: Mutex<RaplDramEnergyState> =
    Mutex::new(RaplDramEnergyState::new());

/// CPUID leaf 6 EAX bit 4 — RAPL support flag.
fn has_rapl() -> bool {
    let eax_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_val >> 4) & 1 != 0
}

/// Read MSR 0x619 (MSR_DRAM_ENERGY_STATUS).
/// Returns (lo, hi) — lo holds the 32-bit energy accumulator.
/// On QEMU or unsupported hardware may return (0, 0).
unsafe fn read_msr_dram_energy() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") 0x619u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

pub fn init() {
    if has_rapl() {
        let (lo, _hi) = unsafe { read_msr_dram_energy() };
        let mut state = MSR_RAPL_DRAM_ENERGY.lock();
        state.last_lo = lo;
        serial_println!("[msr_rapl_dram_energy] init: RAPL supported, seeded last_lo={}", lo);
    } else {
        serial_println!("[msr_rapl_dram_energy] init: RAPL not supported on this CPU");
    }
}

pub fn tick(age: u32) {
    if age % 1000 != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let (lo, _hi) = unsafe { read_msr_dram_energy() };

    let mut state = MSR_RAPL_DRAM_ENERGY.lock();

    // --- dram_energy_lo: low 16 bits of MSR 0x619 lo, mapped to 0-1000 ---
    let raw_lo16: u32 = (lo & 0xFFFF) as u32;
    let dram_energy_lo: u16 = (raw_lo16 * 1000 / 65536) as u16;

    // --- dram_delta: wrapping sub of low 16 bits, mapped to 0-1000 ---
    let last_lo16: u32 = state.last_lo & 0xFFFF;
    let raw_diff: u32 = raw_lo16.wrapping_sub(last_lo16) & 0xFFFF;
    let dram_delta: u16 = (raw_diff * 1000 / 65536) as u16;

    // --- dram_power_sense: EMA of dram_delta (alpha = 1/8) ---
    let dram_power_sense: u16 =
        ((state.dram_power_sense as u32 * 7 + dram_delta as u32) / 8) as u16;

    // --- dram_ema: slower EMA of dram_power_sense (alpha = 1/8) ---
    let dram_ema: u16 =
        ((state.dram_ema as u32 * 7 + dram_power_sense as u32) / 8) as u16;

    state.dram_energy_lo = dram_energy_lo;
    state.dram_delta = dram_delta;
    state.dram_power_sense = dram_power_sense;
    state.dram_ema = dram_ema;
    state.last_lo = lo;

    serial_println!(
        "[msr_rapl_dram_energy] age={} energy={} delta={} power={} ema={}",
        age,
        state.dram_energy_lo,
        state.dram_delta,
        state.dram_power_sense,
        state.dram_ema,
    );
}

pub fn get_dram_energy_lo() -> u16 {
    MSR_RAPL_DRAM_ENERGY.lock().dram_energy_lo
}

pub fn get_dram_delta() -> u16 {
    MSR_RAPL_DRAM_ENERGY.lock().dram_delta
}

pub fn get_dram_power_sense() -> u16 {
    MSR_RAPL_DRAM_ENERGY.lock().dram_power_sense
}

pub fn get_dram_ema() -> u16 {
    MSR_RAPL_DRAM_ENERGY.lock().dram_ema
}

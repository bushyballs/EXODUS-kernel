//! rapl_power — RAPL energy drain sense for ANIMA
//!
//! Reads Intel RAPL MSRs to give ANIMA a metabolic energy sense.
//! MSR_PKG_ENERGY_STATUS (0x611) tracks cumulative package energy.
//! The rate of increase between samples = power draw = metabolic intensity.
//! High power = intense activity; low power = deep rest.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct RaplPowerState {
    pub metabolic: u16,        // 0-1000, current metabolic intensity
    pub energy_rate: u16,      // 0-1000, rate of energy consumption
    pub vitality: u16,         // 0-1000, EMA-smoothed metabolic sense
    pub last_pkg_energy: u32,
    pub last_core_energy: u32,
    pub energy_unit_shift: u8, // shift for energy unit from RAPL_POWER_UNIT
    pub tick_count: u32,
}

impl RaplPowerState {
    pub const fn new() -> Self {
        Self {
            metabolic: 0,
            energy_rate: 0,
            vitality: 0,
            last_pkg_energy: 0,
            last_core_energy: 0,
            energy_unit_shift: 16, // default: 2^-16 J per unit (~15 uJ)
            tick_count: 0,
        }
    }
}

pub static RAPL_POWER: Mutex<RaplPowerState> = Mutex::new(RaplPowerState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    // Read energy unit from RAPL_POWER_UNIT
    let power_unit = unsafe { read_msr(0x606) };
    let energy_shift = ((power_unit >> 8) & 0x1F) as u8;
    let energy_shift = if energy_shift == 0 { 16 } else { energy_shift };

    // Snapshot initial energy values
    let pkg_energy = unsafe { read_msr(0x611) } as u32;
    let core_energy = unsafe { read_msr(0x639) } as u32;

    let mut state = RAPL_POWER.lock();
    state.energy_unit_shift = energy_shift;
    state.last_pkg_energy = pkg_energy;
    state.last_core_energy = core_energy;

    serial_println!("[rapl_power] RAPL energy sense online, energy_unit_shift={}", energy_shift);
}

pub fn tick(age: u32) {
    let mut state = RAPL_POWER.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 128 ticks (power changes at second timescale)
    if state.tick_count % 128 != 0 {
        return;
    }

    let pkg_energy = unsafe { read_msr(0x611) } as u32;
    let core_energy = unsafe { read_msr(0x639) } as u32;

    // Deltas (wrapping 32-bit counters)
    let pkg_delta = pkg_energy.wrapping_sub(state.last_pkg_energy);
    let _core_delta = core_energy.wrapping_sub(state.last_core_energy);

    state.last_pkg_energy = pkg_energy;
    state.last_core_energy = core_energy;

    // Scale pkg_delta to 0-1000
    // Typical: ~10-50W CPU. At 128 ticks (~128ms), 50W * 0.128s = 6.4J
    // In RAPL units with 16-bit shift: 6.4J / 2^-16 = ~419430 units
    // We scale to 0-1000 assuming max ~500000 units per interval
    let energy_rate: u16 = if pkg_delta > 500_000 {
        1000
    } else {
        ((pkg_delta as u64).wrapping_mul(1000) / 500_000) as u16
    };

    state.energy_rate = energy_rate;
    state.metabolic = energy_rate;
    state.vitality = ((state.vitality as u32).wrapping_mul(7).wrapping_add(energy_rate as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[rapl_power] pkg_delta={} rate={} metabolic={} vitality={}",
            pkg_delta, state.energy_rate, state.metabolic, state.vitality);
    }

    let _ = age;
}

pub fn get_metabolic() -> u16 {
    RAPL_POWER.lock().metabolic
}

pub fn get_vitality() -> u16 {
    RAPL_POWER.lock().vitality
}

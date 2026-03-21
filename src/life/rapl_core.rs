#![allow(dead_code)]

use crate::sync::Mutex;

const MSR_PP0_ENERGY_STATUS: u32 = 0x639;
const MSR_PP1_ENERGY_STATUS: u32 = 0x641;

pub struct RaplCoreState {
    pub core_power: u16,          // CPU core domain energy rate
    pub sense_power: u16,         // uncore/GPU domain energy rate
    pub compute_vs_sense: u16,    // 0=sense-dominant, 500=balanced, 1000=compute-dominant
    pub total_domain_energy: u16, // combined domain energy
    prev_pp0: u32,
    prev_pp1: u32,
    tick_count: u32,
}

impl RaplCoreState {
    const fn new() -> Self {
        Self {
            core_power: 0,
            sense_power: 0,
            compute_vs_sense: 500,
            total_domain_energy: 0,
            prev_pp0: 0,
            prev_pp1: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<RaplCoreState> = Mutex::new(RaplCoreState::new());

unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let mut state = MODULE.lock();
    let pp0_raw = unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) } as u32;
    let pp1_raw = unsafe { rdmsr(MSR_PP1_ENERGY_STATUS) } as u32;
    state.prev_pp0 = pp0_raw & 0xFFFF_FFFF;
    state.prev_pp1 = pp1_raw & 0xFFFF_FFFF;
    state.tick_count = 0;
    serial_println!("[rapl_core] init: pp0=0x{:08x} pp1=0x{:08x}", state.prev_pp0, state.prev_pp1);
}

pub fn tick(age: u32) {
    if age % 20 != 0 {
        return;
    }

    let pp0_raw = (unsafe { rdmsr(MSR_PP0_ENERGY_STATUS) }) as u32;
    let pp1_raw = (unsafe { rdmsr(MSR_PP1_ENERGY_STATUS) }) as u32;

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    let pp0_current = pp0_raw & 0xFFFF_FFFF;
    let pp1_current = pp1_raw & 0xFFFF_FFFF;

    // Wrapping delta to handle counter rollover
    let core_delta_raw = pp0_current.wrapping_sub(state.prev_pp0) & 0xFFFF_FFFF;
    let uncore_delta_raw = pp1_current.wrapping_sub(state.prev_pp1) & 0xFFFF_FFFF;

    state.prev_pp0 = pp0_current;
    state.prev_pp1 = pp1_current;

    // Cap deltas at u16 max before normalizing to 0-1000
    let core_capped: u32 = if core_delta_raw > 65535 { 65535 } else { core_delta_raw };
    let uncore_capped: u32 = if uncore_delta_raw > 65535 { 65535 } else { uncore_delta_raw };

    // Normalize to 0-1000 scale
    let core_signal: u16 = (core_capped * 1000 / 65535) as u16;
    let uncore_signal: u16 = (uncore_capped * 1000 / 65535) as u16;

    // EMA: (old * 7 + signal) / 8
    let new_core = ((state.core_power as u32 * 7) + core_signal as u32) / 8;
    let new_sense = ((state.sense_power as u32 * 7) + uncore_signal as u32) / 8;

    state.core_power = new_core.min(1000) as u16;
    state.sense_power = new_sense.min(1000) as u16;

    // compute_vs_sense ratio
    state.compute_vs_sense = if core_capped == 0 && uncore_capped == 0 {
        500
    } else if core_capped > uncore_capped {
        let diff = core_capped - uncore_capped;
        let bonus = (diff * 250 / 65535).min(250);
        (750_u32 + bonus).min(1000) as u16
    } else if uncore_capped > core_capped {
        let diff = uncore_capped - core_capped;
        let penalty = (diff * 250 / 65535).min(250);
        250_u32.saturating_sub(penalty) as u16
    } else {
        500
    };

    // total_domain_energy: average of core and sense, capped 1000
    let total = ((state.core_power as u32) + (state.sense_power as u32)) / 2;
    state.total_domain_energy = total.min(1000) as u16;

    serial_println!(
        "[rapl_core] tick={} core={} sense={} cvs={} total={}",
        state.tick_count,
        state.core_power,
        state.sense_power,
        state.compute_vs_sense,
        state.total_domain_energy
    );
}

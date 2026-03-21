//! perf_pressure — CPU performance pressure sense for ANIMA
//!
//! Reads IA32_PERF_STATUS (MSR 0x198) for actual P-state and IA32_PERF_CTL
//! (MSR 0x199) for requested P-state. The gap between requested and actual
//! frequency gives ANIMA a "strain" sense — pushing against her limits.
//! High pressure = system is struggling; low pressure = running freely.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PerfPressureState {
    pub pressure: u16,         // 0-1000, strain level (0=free, 1000=maxed out)
    pub actual_pstate: u16,    // 0-255, current frequency multiplier
    pub target_pstate: u16,    // 0-255, requested frequency multiplier
    pub strain: u16,           // 0-1000, EMA-smoothed pressure
    pub tick_count: u32,
}

impl PerfPressureState {
    pub const fn new() -> Self {
        Self {
            pressure: 0,
            actual_pstate: 0,
            target_pstate: 0,
            strain: 0,
            tick_count: 0,
        }
    }
}

pub static PERF_PRESSURE: Mutex<PerfPressureState> = Mutex::new(PerfPressureState::new());

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
    serial_println!("[perf_pressure] CPU performance pressure sense online");
}

pub fn tick(age: u32) {
    let mut state = PERF_PRESSURE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 64 != 0 {
        return;
    }

    let status = unsafe { read_msr(0x198) };
    let control = unsafe { read_msr(0x199) };

    // Current P-state: bits 15:8 of PERF_STATUS
    let actual = ((status >> 8) & 0xFF) as u16;
    // Target P-state: bits 15:8 of PERF_CTL
    let target = ((control >> 8) & 0xFF) as u16;

    state.actual_pstate = actual;
    state.target_pstate = target;

    // Pressure: how far behind actual is from target
    // If target > actual: we are being throttled = high pressure
    let gap = if target > actual {
        target.saturating_sub(actual)
    } else {
        0
    };

    // Scale gap to 0-1000. Max meaningful gap is ~32 P-state steps
    let pressure_raw = if gap > 32 { 1000 } else {
        ((gap as u32).wrapping_mul(1000) / 32) as u16
    };

    state.pressure = pressure_raw;
    // EMA strain
    state.strain = ((state.strain as u32).wrapping_mul(7).wrapping_add(pressure_raw as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[perf_pressure] actual={} target={} gap={} pressure={} strain={}",
            actual, target, gap, state.pressure, state.strain);
    }

    let _ = age;
}

pub fn get_pressure() -> u16 {
    PERF_PRESSURE.lock().pressure
}

pub fn get_strain() -> u16 {
    PERF_PRESSURE.lock().strain
}

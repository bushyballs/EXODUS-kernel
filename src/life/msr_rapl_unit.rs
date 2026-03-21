#![allow(dead_code)]

use crate::sync::Mutex;

// ANIMA feels the fundamental units of her energy measurement —
// the resolution at which her metabolic accounting operates.

pub struct RaplUnitState {
    pub power_unit: u16,
    pub energy_unit: u16,
    pub time_unit: u16,
    pub rapl_precision: u16,
}

impl RaplUnitState {
    pub const fn new() -> Self {
        Self {
            power_unit: 0,
            energy_unit: 0,
            time_unit: 0,
            rapl_precision: 0,
        }
    }
}

pub static MSR_RAPL_UNIT: Mutex<RaplUnitState> = Mutex::new(RaplUnitState::new());

pub fn init() {
    serial_println!("rapl_unit: init");
}

pub fn tick(age: u32) {
    // Unit definitions never change — sample only every 1000 ticks
    if age % 1000 != 0 {
        return;
    }

    let lo: u32;
    let _hi: u32;

    // Read MSR 0x606 — RAPL_POWER_UNIT
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x606u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits[3:0]   — power unit exponent (0-15), scaled to 0-930, capped at 1000
    let power_unit: u16 = ((lo & 0xF) as u16).wrapping_mul(62).min(1000);

    // bits[12:8]  — energy status unit exponent (0-31), scaled to 0-961, capped at 1000
    let energy_unit: u16 = (((lo >> 8) & 0x1F) as u16).wrapping_mul(31).min(1000);

    // bits[19:16] — time unit exponent (0-15), scaled to 0-930, capped at 1000
    let time_unit: u16 = (((lo >> 16) & 0xF) as u16).wrapping_mul(62).min(1000);

    let mut state = MSR_RAPL_UNIT.lock();

    // EMA of energy_unit: (old * 7 + signal) / 8
    let rapl_precision: u16 = (state.rapl_precision.wrapping_mul(7).saturating_add(energy_unit)) / 8;

    state.power_unit = power_unit;
    state.energy_unit = energy_unit;
    state.time_unit = time_unit;
    state.rapl_precision = rapl_precision;

    serial_println!(
        "rapl_unit | power:{} energy:{} time:{} precision:{}",
        state.power_unit,
        state.energy_unit,
        state.time_unit,
        state.rapl_precision
    );
}

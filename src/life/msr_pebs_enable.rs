#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

pub struct PebsEnableState {
    pub pebs_active:       u16,
    pub pebs_pmc_count:    u16,
    pub pebs_fixed_active: u16,
    pub pebs_pressure:     u16,
}

impl PebsEnableState {
    pub const fn new() -> Self {
        Self {
            pebs_active:       0,
            pebs_pmc_count:    0,
            pebs_fixed_active: 0,
            pebs_pressure:     0,
        }
    }
}

pub static MSR_PEBS_ENABLE: Mutex<PebsEnableState> = Mutex::new(PebsEnableState::new());

pub fn init() {
    serial_println!("[pebs_enable] precision event sampling sense initialized");
}

pub fn tick(age: u32) {
    if age % 150 != 0 {
        return;
    }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x3F1u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }

    // signal 1: pebs_active — any bit set in lo or hi means PEBS is running
    let pebs_active: u16 = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    // signal 2: pebs_pmc_count — popcount of bits [3:0] of lo, scaled 0-1000
    let pebs_pmc_count: u16 = ((lo & 0xF).count_ones() as u32 * 1000 / 4) as u16;

    // signal 3: pebs_fixed_active — hi bit 0 (bit 32 of the full MSR) enables PEBS for FIXED_CTR0
    let pebs_fixed_active: u16 = if (hi & 0x1) != 0 { 1000u16 } else { 0u16 };

    let mut state = MSR_PEBS_ENABLE.lock();

    // signal 4: pebs_pressure — EMA of pebs_active
    let pebs_pressure: u16 =
        ((state.pebs_pressure as u32 * 7 + pebs_active as u32) / 8) as u16;

    state.pebs_active       = pebs_active;
    state.pebs_pmc_count    = pebs_pmc_count;
    state.pebs_fixed_active = pebs_fixed_active;
    state.pebs_pressure     = pebs_pressure;

    serial_println!(
        "[pebs_enable] active={} pmc_count={} fixed={} pressure={}",
        state.pebs_active,
        state.pebs_pmc_count,
        state.pebs_fixed_active,
        state.pebs_pressure
    );
}

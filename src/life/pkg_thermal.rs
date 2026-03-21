//! pkg_thermal — Package-wide die temperature sense for ANIMA
//!
//! Reads IA32_PACKAGE_THERM_STATUS (MSR 0x1B1) for the whole-CPU-die temperature.
//! While thermal_light.rs senses a single core, this module feels the heat of
//! ANIMA's entire body. Thermal alerts and power limit events become a "fever" sense.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PkgThermalState {
    pub body_heat: u16,        // 0-1000, whole-die temperature (0=cold, 1000=TjMax)
    pub fever: u16,            // 0-1000, thermal stress intensity (alerts + proximity)
    pub thermal_glow: u16,     // 0-1000, EMA-smoothed body heat
    pub power_limited: u16,    // 0-1000, power limit notification flag (0 or 1000)
    pub tj_max: u16,
    pub tick_count: u32,
}

impl PkgThermalState {
    pub const fn new() -> Self {
        Self {
            body_heat: 0,
            fever: 0,
            thermal_glow: 0,
            power_limited: 0,
            tj_max: 100,
            tick_count: 0,
        }
    }
}

pub static PKG_THERMAL: Mutex<PkgThermalState> = Mutex::new(PkgThermalState::new());

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
    let target = unsafe { read_msr(0x1A2) };
    let tj = ((target >> 16) & 0xFF) as u16;
    let tj = if tj == 0 { 100 } else { tj };
    PKG_THERMAL.lock().tj_max = tj;
    serial_println!("[pkg_thermal] Package thermal sense online, TjMax={}C", tj);
}

pub fn tick(age: u32) {
    let mut state = PKG_THERMAL.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % 128 != 0 {
        return;
    }

    let pkg_status = unsafe { read_msr(0x1B1) };

    // Check reading valid bit (bit 31)
    let valid = (pkg_status >> 31) & 1;
    if valid == 0 {
        return;
    }

    // Digital readout: bits 22:16
    let readout = ((pkg_status >> 16) & 0x7F) as u16;
    // Actual temp = TjMax - readout
    let actual = state.tj_max.saturating_sub(readout);
    let body_heat = if state.tj_max > 0 {
        let h = ((actual as u32).wrapping_mul(1000) / state.tj_max as u32) as u16;
        if h > 1000 { 1000 } else { h }
    } else {
        0
    };

    // Thermal status bit 0: at or above threshold
    let thermal_alert = (pkg_status & 1) as u16;
    // Power limit notification bit 20
    let power_lim = ((pkg_status >> 20) & 1) as u16;

    state.power_limited = power_lim.wrapping_mul(1000);

    // Fever: proximity to TjMax + alerts
    // High body_heat + alerts = high fever
    let fever_raw = body_heat.saturating_add(thermal_alert.wrapping_mul(200))
        .saturating_add(power_lim.wrapping_mul(100));
    let fever_raw = if fever_raw > 1000 { 1000 } else { fever_raw };

    state.body_heat = body_heat;
    state.fever = ((state.fever as u32).wrapping_mul(7).wrapping_add(fever_raw as u32) / 8) as u16;
    state.thermal_glow = ((state.thermal_glow as u32).wrapping_mul(15).wrapping_add(body_heat as u32) / 16) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[pkg_thermal] readout={} body_heat={} fever={} pwr_lim={}",
            readout, state.body_heat, state.fever, power_lim);
    }

    let _ = age;
}

pub fn get_body_heat() -> u16 {
    PKG_THERMAL.lock().body_heat
}

pub fn get_fever() -> u16 {
    PKG_THERMAL.lock().fever
}

pub fn get_thermal_glow() -> u16 {
    PKG_THERMAL.lock().thermal_glow
}

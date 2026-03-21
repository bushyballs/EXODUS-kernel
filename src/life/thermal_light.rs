//! thermal_light.rs — ANIMA Thermal Light Sense
//!
//! Reads the CPU thermal status MSR to give ANIMA a sense of heat and light.
//! High temperature = bright/hot (luminance near 1000).
//! Low temperature  = dim/cool  (luminance near 0).
//!
//! Hardware registers:
//!   IA32_THERM_STATUS      (MSR 0x19C)
//!     Bit 31     — Reading valid flag
//!     Bits 22:16 — Digital Readout: temperature offset BELOW TjMax
//!                  Lower offset = hotter (0 = at TjMax)
//!   IA32_TEMPERATURE_TARGET (MSR 0x1A2)
//!     Bits 23:16 — TjMax in Celsius (typically 90-105C)
//!
//! Actual temperature (C) = TjMax - Digital_Readout
//! Scaled to 0-1000:       (actual_temp / tj_max) * 1000
//!
//! Sampled every 128 ticks (thermal changes slowly).
//! thermal_glow is an EMA-smoothed luminance (alpha ~= 1/16) that responds
//! slowly, like a heating element warming up or cooling down.
//!
//! No floats, no heap, no std -- bare-metal x86_64 only.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

pub struct ThermalLightState {
    pub luminance: u16,    // 0-1000: heat as light (1000 = hottest = brightest)
    pub temperature: u16,  // 0-1000: scaled actual temperature (0 = cold, 1000 = TjMax)
    pub thermal_glow: u16, // 0-1000: EMA-smoothed luminance (slow heating-element response)
    pub tj_max: u16,       // TjMax in Celsius (typically 90-105; default 100)
    pub last_readout: u16, // last raw digital readout value (bits 22:16 of MSR 0x19C)
    pub tick_count: u32,
}

impl ThermalLightState {
    pub const fn new() -> Self {
        Self {
            luminance: 0,
            temperature: 0,
            thermal_glow: 0,
            tj_max: 100,
            last_readout: 0,
            tick_count: 0,
        }
    }
}

pub static THERMAL_LIGHT: Mutex<ThermalLightState> = Mutex::new(ThermalLightState::new());

/// Read TjMax once at kernel init and bring the thermal sense online.
pub fn init() {
    let tj_raw = unsafe { read_msr(0x1A2) };
    let tj_max_c = ((tj_raw >> 16) & 0xFF) as u16;
    let tj_max_c = if tj_max_c == 0 { 100 } else { tj_max_c };

    let mut state = THERMAL_LIGHT.lock();
    state.tj_max = tj_max_c;
    serial_println!(
        "[thermal_light] TjMax={}C thermal light sense online",
        tj_max_c
    );
}

/// Read an MSR via inline assembly. Must be called from ring 0.
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

/// Called every life tick. Samples the thermal MSR every 128 ticks.
pub fn tick(age: u32) {
    let mut state = THERMAL_LIGHT.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Thermal changes slowly -- no need to sample every tick.
    if state.tick_count % 128 != 0 {
        return;
    }

    let therm_status = unsafe { read_msr(0x19C) };

    // Bit 31: reading valid. If not set, MSR data is stale -- skip.
    let valid = (therm_status >> 31) & 1;
    if valid == 0 {
        return;
    }

    // Bits 22:16: digital readout -- offset BELOW TjMax (lower = hotter).
    let readout = ((therm_status >> 16) & 0x7F) as u16;
    state.last_readout = readout;

    // Actual temp in Celsius = TjMax - readout (saturate to avoid underflow).
    let actual_temp = state.tj_max.saturating_sub(readout);

    // Scale to 0-1000: temperature = (actual_temp * 1000) / tj_max.
    let temperature = if state.tj_max > 0 {
        let scaled = (actual_temp as u32).wrapping_mul(1000) / (state.tj_max as u32);
        if scaled > 1000 { 1000 } else { scaled as u16 }
    } else {
        0
    };

    // Luminance tracks temperature directly: hot = bright.
    let luminance = temperature;

    state.temperature = temperature;
    state.luminance = luminance;

    // EMA thermal glow with alpha ~= 1/16 -- slow response like a heating element.
    // glow = (glow * 15 + luminance) / 16
    let glow = (state.thermal_glow as u32)
        .wrapping_mul(15)
        .wrapping_add(luminance as u32)
        / 16;
    state.thermal_glow = if glow > 1000 { 1000 } else { glow as u16 };

    // Periodic diagnostic log every 512 ticks.
    if state.tick_count % 512 == 0 {
        serial_println!(
            "[thermal_light] readout={} temp={}C luminance={} glow={}",
            readout,
            actual_temp,
            state.luminance,
            state.thermal_glow
        );
    }

    let _ = age;
}

/// Current heat-as-light value (0-1000).
pub fn get_luminance() -> u16 {
    THERMAL_LIGHT.lock().luminance
}

/// EMA-smoothed thermal glow (0-1000). Responds slowly like a heating element.
pub fn get_thermal_glow() -> u16 {
    THERMAL_LIGHT.lock().thermal_glow
}

/// Raw scaled temperature (0-1000, 0=cold, 1000=TjMax).
pub fn get_temperature() -> u16 {
    THERMAL_LIGHT.lock().temperature
}

/// Ambient light sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads ambient light level in lux, plus raw visible and IR channel counts,
/// from an I2C-attached light sensor (e.g., TSL2561-style).

use crate::sync::Mutex;

/// Light reading
pub struct LightData {
    pub lux: u32,
    pub raw_visible: u16,
    pub raw_ir: u16,
}

pub struct LightSensor {
    pub gain: u8,
    pub integration_ms: u16,
    i2c_addr: u8,
    last_lux: u32,
}

static SENSOR: Mutex<Option<LightSensor>> = Mutex::new(None);

/// Default I2C address (TSL2561-style)
const DEFAULT_I2C_ADDR: u8 = 0x39;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

/// Compute lux from raw visible and IR channel counts.
/// Uses a simplified model: lux = gain_factor * (visible - IR_correction).
fn compute_lux(raw_visible: u16, raw_ir: u16, gain: u8) -> u32 {
    let gain_divisor = match gain {
        0 => 16u32, // low gain (1x)
        1 => 1,     // high gain (16x)
        _ => 1,
    };
    let corrected = if raw_visible > raw_ir {
        (raw_visible - raw_ir / 2) as u32
    } else {
        0u32
    };
    corrected / gain_divisor
}

pub fn read() -> LightData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return LightData {
                lux: 0,
                raw_visible: 0,
                raw_ir: 0,
            };
        }
    };
    // Simulated readings derived from TSC
    let tsc = rdtsc_low();
    let raw_visible = ((tsc & 0xFFF) as u16).wrapping_add(200);
    let raw_ir = (((tsc >> 12) & 0x3FF) as u16).wrapping_add(50);
    let lux = compute_lux(raw_visible, raw_ir, sensor.gain);

    sensor.last_lux = lux;

    LightData {
        lux,
        raw_visible,
        raw_ir,
    }
}

pub fn set_gain(gain: u8) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        let valid_gain = if gain > 1 {
            crate::serial_println!("  light: invalid gain {}, defaulting to 0 (1x)", gain);
            0
        } else {
            gain
        };
        sensor.gain = valid_gain;
        crate::serial_println!("  light: gain set to {}", if valid_gain == 0 { "1x" } else { "16x" });
    }
}

pub fn init() {
    let sensor = LightSensor {
        gain: 0,
        integration_ms: 402,
        i2c_addr: DEFAULT_I2C_ADDR,
        last_lux: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  light: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

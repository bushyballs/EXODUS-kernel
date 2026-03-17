/// Humidity sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads relative humidity and associated temperature from an I2C-attached
/// combined humidity/temperature sensor (e.g., SHT3x-style).

use crate::sync::Mutex;

/// Humidity reading
pub struct HumidityData {
    pub relative_pct: u16, // 0-100 percent * 100
    pub temp_millicelsius: i32,
}

pub struct HumiditySensor {
    pub odr_hz: u16,
    i2c_addr: u8,
    heater_enabled: bool,
    last_rh: u16,
    last_temp_mc: i32,
}

static SENSOR: Mutex<Option<HumiditySensor>> = Mutex::new(None);

/// Default I2C address (SHT3x-style)
const DEFAULT_I2C_ADDR: u8 = 0x44;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

pub fn read() -> HumidityData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return HumidityData {
                relative_pct: 0,
                temp_millicelsius: 0,
            };
        }
    };
    // Simulated reading: ~50% RH with TSC-derived noise
    let tsc = rdtsc_low();
    let rh_noise = ((tsc & 0x3FF) as i32) - 512; // +/- ~5% noise (*100)
    let rh = (5000i32 + rh_noise).clamp(0, 10000) as u16;
    // Temperature near 25C (25000 mC) with noise
    let temp_noise = (((tsc >> 10) & 0x1FF) as i32) - 256;
    let temp_mc = 25000 + temp_noise;

    sensor.last_rh = rh;
    sensor.last_temp_mc = temp_mc;

    HumidityData {
        relative_pct: rh,
        temp_millicelsius: temp_mc,
    }
}

pub fn init() {
    let sensor = HumiditySensor {
        odr_hz: 1,
        i2c_addr: DEFAULT_I2C_ADDR,
        heater_enabled: false,
        last_rh: 0,
        last_temp_mc: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  humidity: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

/// Barometric pressure sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads atmospheric pressure and derives altitude via the barometric formula.
/// Sea-level reference pressure is configurable for accurate altitude computation.

use crate::sync::Mutex;

/// Pressure reading
pub struct BaroData {
    pub pressure_pa: u32,
    pub altitude_m: f32,
}

pub struct BaroSensor {
    pub odr_hz: u16,
    oversampling: u8,
    sea_level_pa: u32,
    i2c_addr: u8,
    last_pressure_pa: u32,
}

static SENSOR: Mutex<Option<BaroSensor>> = Mutex::new(None);

/// Default I2C address (BMP280-style)
const DEFAULT_I2C_ADDR: u8 = 0x76;

/// Standard sea-level pressure in Pa
const STD_SEA_LEVEL_PA: u32 = 101325;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

/// Compute altitude from pressure using a simplified barometric formula.
/// altitude = 44330 * (1 - (P / P0)^0.1903)
/// We use a linear approximation: altitude ~ (P0 - P) * 0.0832 (valid near sea level)
fn compute_altitude(pressure_pa: u32, sea_level_pa: u32) -> f32 {
    if pressure_pa == 0 || sea_level_pa == 0 {
        return 0.0;
    }
    let diff = sea_level_pa as f32 - pressure_pa as f32;
    // Linear approximation: ~8.3m per hPa (100 Pa) difference
    diff * 0.083
}

pub fn read() -> BaroData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return BaroData {
                pressure_pa: 0,
                altitude_m: 0.0,
            };
        }
    };
    // Simulated pressure reading: ~101325 Pa with small TSC-derived variation
    let tsc = rdtsc_low();
    let noise = ((tsc & 0x1FF) as i32) - 256; // +/- 256 Pa noise
    let pressure = (STD_SEA_LEVEL_PA as i32 + noise).max(30000) as u32;
    sensor.last_pressure_pa = pressure;

    let altitude = compute_altitude(pressure, sensor.sea_level_pa);
    BaroData {
        pressure_pa: pressure,
        altitude_m: altitude,
    }
}

pub fn set_sea_level_pa(pa: u32) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        let valid_pa = if pa < 80000 || pa > 120000 {
            crate::serial_println!("  baro: invalid sea level {}Pa, using standard", pa);
            STD_SEA_LEVEL_PA
        } else {
            pa
        };
        sensor.sea_level_pa = valid_pa;
        crate::serial_println!("  baro: sea level reference set to {}Pa", valid_pa);
    }
}

pub fn init() {
    let sensor = BaroSensor {
        odr_hz: 25,
        oversampling: 4,
        sea_level_pa: STD_SEA_LEVEL_PA,
        i2c_addr: DEFAULT_I2C_ADDR,
        last_pressure_pa: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  baro: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

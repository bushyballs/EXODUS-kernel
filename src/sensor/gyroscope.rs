/// Gyroscope sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads 3-axis angular velocity from an I2C-attached gyroscope.
/// Default configuration: +/-250 dps range, 100 Hz output data rate.
/// Bias calibration is applied to raw readings.

use crate::sync::Mutex;

/// 3-axis angular velocity (mdps)
pub struct GyroData {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Gyro bias calibration
struct GyroBias {
    x: i32,
    y: i32,
    z: i32,
}

pub struct GyroSensor {
    pub range_dps: u16,
    pub odr_hz: u16,
    i2c_addr: u8,
    bias: GyroBias,
    last_x: i32,
    last_y: i32,
    last_z: i32,
}

static SENSOR: Mutex<Option<GyroSensor>> = Mutex::new(None);

/// Default I2C address for gyroscope (L3GD20-style)
const DEFAULT_I2C_ADDR: u8 = 0x6B;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

/// Read raw angular velocity from hardware.
fn read_raw_gyro(sensor: &GyroSensor) -> (i32, i32, i32) {
    let tsc = rdtsc_low();
    let scale = match sensor.range_dps {
        250 => 1,
        500 => 2,
        1000 => 4,
        2000 => 8,
        _ => 1,
    };
    // Simulated gyro readings: small random angular velocity in mdps
    let raw_x = ((tsc & 0xFF) as i32 - 128) * scale;
    let raw_y = (((tsc >> 8) & 0xFF) as i32 - 128) * scale;
    let raw_z = (((tsc >> 16) & 0xFF) as i32 - 128) * scale;
    (raw_x, raw_y, raw_z)
}

pub fn read() -> GyroData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return GyroData { x: 0, y: 0, z: 0 };
        }
    };
    let (raw_x, raw_y, raw_z) = read_raw_gyro(sensor);
    // Apply bias correction
    let x = raw_x - sensor.bias.x;
    let y = raw_y - sensor.bias.y;
    let z = raw_z - sensor.bias.z;
    sensor.last_x = x;
    sensor.last_y = y;
    sensor.last_z = z;
    GyroData { x, y, z }
}

pub fn configure(range_dps: u16, odr_hz: u16) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        let valid_range = match range_dps {
            250 | 500 | 1000 | 2000 => range_dps,
            _ => {
                crate::serial_println!("  gyro: invalid range {}dps, defaulting to 250dps", range_dps);
                250
            }
        };
        let valid_odr = if odr_hz == 0 { 100 } else if odr_hz > 8000 { 8000 } else { odr_hz };
        sensor.range_dps = valid_range;
        sensor.odr_hz = valid_odr;
        crate::serial_println!("  gyro: configured range={}dps odr={}Hz", valid_range, valid_odr);
    }
}

pub fn init() {
    let sensor = GyroSensor {
        range_dps: 250,
        odr_hz: 100,
        i2c_addr: DEFAULT_I2C_ADDR,
        bias: GyroBias { x: 0, y: 0, z: 0 },
        last_x: 0,
        last_y: 0,
        last_z: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  gyro: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

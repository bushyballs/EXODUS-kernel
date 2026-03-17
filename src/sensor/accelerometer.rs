/// Accelerometer sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads 3-axis acceleration data from an I2C-attached accelerometer.
/// Default configuration: +/-2g range, 100 Hz output data rate.
/// Calibration offsets are stored and applied to raw readings.

use crate::sync::Mutex;

/// 3-axis acceleration data (mg)
pub struct AccelData {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Calibration offsets applied to raw readings
struct CalibrationData {
    offset_x: i32,
    offset_y: i32,
    offset_z: i32,
}

pub struct AccelSensor {
    pub range_g: u8,
    pub odr_hz: u16,
    i2c_addr: u8,
    calibration: CalibrationData,
    last_x: i32,
    last_y: i32,
    last_z: i32,
}

static SENSOR: Mutex<Option<AccelSensor>> = Mutex::new(None);

/// Default I2C address for accelerometer (LIS2DH-style)
const DEFAULT_I2C_ADDR: u8 = 0x19;

/// Simulate reading raw acceleration from hardware registers.
/// In a real driver this would perform I2C transactions via MMIO.
fn read_raw_accel(sensor: &AccelSensor) -> (i32, i32, i32) {
    // Read TSC for pseudo-sensor data (no real hardware attached yet).
    // When real I2C is available, this reads registers 0x28..0x2D.
    let tsc = rdtsc_low();
    let scale = match sensor.range_g {
        2 => 1,
        4 => 2,
        8 => 4,
        16 => 8,
        _ => 1,
    };
    // Simulated sensor noise derived from TSC bits
    let raw_x = ((tsc & 0xFF) as i32 - 128) * scale;
    let raw_y = (((tsc >> 8) & 0xFF) as i32 - 128) * scale;
    // Z axis has 1g bias (earth gravity) ~ 1000 mg
    let raw_z = (((tsc >> 16) & 0xFF) as i32 - 128) * scale + 1000;
    (raw_x, raw_y, raw_z)
}

/// Get low 32 bits of TSC for timing/pseudo-data
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

pub fn read() -> AccelData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return AccelData { x: 0, y: 0, z: 0 };
        }
    };
    let (raw_x, raw_y, raw_z) = read_raw_accel(sensor);
    let x = raw_x + sensor.calibration.offset_x;
    let y = raw_y + sensor.calibration.offset_y;
    let z = raw_z + sensor.calibration.offset_z;
    sensor.last_x = x;
    sensor.last_y = y;
    sensor.last_z = z;
    AccelData { x, y, z }
}

pub fn configure(range_g: u8, odr_hz: u16) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        // Validate range: only 2, 4, 8, 16 g are supported
        let valid_range = match range_g {
            2 | 4 | 8 | 16 => range_g,
            _ => {
                crate::serial_println!("  accel: invalid range {}g, defaulting to 2g", range_g);
                2
            }
        };
        // Clamp ODR to 1..5000 Hz
        let valid_odr = if odr_hz == 0 { 100 } else if odr_hz > 5000 { 5000 } else { odr_hz };
        sensor.range_g = valid_range;
        sensor.odr_hz = valid_odr;
        crate::serial_println!("  accel: configured range={}g odr={}Hz", valid_range, valid_odr);
    }
}

pub fn init() {
    let sensor = AccelSensor {
        range_g: 2,
        odr_hz: 100,
        i2c_addr: DEFAULT_I2C_ADDR,
        calibration: CalibrationData {
            offset_x: 0,
            offset_y: 0,
            offset_z: 0,
        },
        last_x: 0,
        last_y: 0,
        last_z: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  accel: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

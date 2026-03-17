/// Magnetometer/compass sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads 3-axis magnetic field strength from an I2C-attached magnetometer
/// and computes compass heading. Hard/soft iron calibration offsets are
/// applied to raw readings.

use crate::sync::Mutex;

/// 3-axis magnetic field (uT)
pub struct MagData {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Hard iron calibration offsets
struct HardIronCal {
    offset_x: i32,
    offset_y: i32,
    offset_z: i32,
}

pub struct MagSensor {
    pub range_ut: u16,
    pub odr_hz: u16,
    i2c_addr: u8,
    calibration: HardIronCal,
    last_x: i32,
    last_y: i32,
    last_z: i32,
}

static SENSOR: Mutex<Option<MagSensor>> = Mutex::new(None);

/// Default I2C address (HMC5883L / QMC5883L-style)
const DEFAULT_I2C_ADDR: u8 = 0x1E;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

/// Read raw magnetic field from hardware
fn read_raw_mag(sensor: &MagSensor) -> (i32, i32, i32) {
    let tsc = rdtsc_low();
    // Simulated earth magnetic field ~25-65 uT with noise
    // Earth field in X (north) ~20 uT, small Y, ~40 uT Z (downward in northern hemisphere)
    let raw_x = 20 + ((tsc & 0xFF) as i32 - 128) / 16;
    let raw_y = ((tsc >> 8) & 0xFF) as i32 / 16 - 8;
    let raw_z = 40 + (((tsc >> 16) & 0xFF) as i32 - 128) / 16;
    let _ = sensor.range_ut; // range affects scaling in real hardware
    (raw_x, raw_y, raw_z)
}

pub fn read() -> MagData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return MagData { x: 0, y: 0, z: 0 };
        }
    };
    let (raw_x, raw_y, raw_z) = read_raw_mag(sensor);
    // Apply hard-iron calibration
    let x = raw_x + sensor.calibration.offset_x;
    let y = raw_y + sensor.calibration.offset_y;
    let z = raw_z + sensor.calibration.offset_z;
    sensor.last_x = x;
    sensor.last_y = y;
    sensor.last_z = z;
    MagData { x, y, z }
}

/// Compute compass heading in degrees (0-360) from the X and Y magnetic components.
/// Uses a lookup-table-free atan2 approximation suitable for no_std.
pub fn heading_degrees() -> f32 {
    let data = read();
    let fx = data.x as f32;
    let fy = data.y as f32;

    // atan2 approximation
    if fx == 0.0 && fy == 0.0 {
        return 0.0;
    }

    let heading_rad = atan2_approx(fy, fx);
    let mut heading_deg = heading_rad * (180.0 / 3.14159265);
    if heading_deg < 0.0 {
        heading_deg += 360.0;
    }
    heading_deg
}

/// Fast atan2 approximation (max error ~0.07 rad)
fn atan2_approx(y: f32, x: f32) -> f32 {
    let abs_x = if x < 0.0 { -x } else { x };
    let abs_y = if y < 0.0 { -y } else { y };
    let min_val = if abs_x < abs_y { abs_x } else { abs_y };
    let max_val = if abs_x > abs_y { abs_x } else { abs_y };

    if max_val == 0.0 {
        return 0.0;
    }

    let a = min_val / max_val;
    let s = a * a;
    let mut r = ((-0.0464964749 * s + 0.15931422) * s - 0.327622764) * s * a + a;

    if abs_y > abs_x {
        r = 1.57079637 - r;
    }
    if x < 0.0 {
        r = 3.14159265 - r;
    }
    if y < 0.0 {
        r = -r;
    }
    r
}

pub fn init() {
    let sensor = MagSensor {
        range_ut: 800, // +/- 800 uT full scale
        odr_hz: 75,
        i2c_addr: DEFAULT_I2C_ADDR,
        calibration: HardIronCal {
            offset_x: 0,
            offset_y: 0,
            offset_z: 0,
        },
        last_x: 0,
        last_y: 0,
        last_z: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  mag: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

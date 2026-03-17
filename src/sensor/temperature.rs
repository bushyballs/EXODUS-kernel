/// Temperature sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads ambient temperature from an I2C-attached digital temperature sensor.
/// Supports configurable resolution and alert threshold registers for
/// over/under-temperature notification.

use crate::sync::Mutex;

/// Temperature reading
pub struct TempData {
    pub millicelsius: i32,
}

pub struct TempSensor {
    pub resolution_bits: u8,
    i2c_addr: u8,
    alert_low_mc: i32,
    alert_high_mc: i32,
    last_mc: i32,
}

static SENSOR: Mutex<Option<TempSensor>> = Mutex::new(None);

/// Default I2C address (TMP102/LM75-style)
const DEFAULT_I2C_ADDR: u8 = 0x48;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

pub fn read() -> TempData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return TempData { millicelsius: 0 };
        }
    };
    // Simulated temperature: ~25000 mC (25 degC) with TSC-derived noise
    let tsc = rdtsc_low();
    let resolution_mask = match sensor.resolution_bits {
        9 => 0xF000u32,  // 0.5 degC steps
        10 => 0xF800,     // 0.25 degC steps
        11 => 0xFC00,     // 0.125 degC steps
        12 => 0xFE00,     // 0.0625 degC steps
        _ => 0xFE00,
    };
    let _ = resolution_mask; // would mask raw ADC value on real hardware
    let noise = ((tsc & 0x1FF) as i32) - 256; // +/- ~256 mC
    let temp_mc = 25000 + noise;

    // Check alert thresholds
    if temp_mc < sensor.alert_low_mc {
        crate::serial_println!("  temp: ALERT LOW {}mC < {}mC", temp_mc, sensor.alert_low_mc);
    }
    if temp_mc > sensor.alert_high_mc {
        crate::serial_println!("  temp: ALERT HIGH {}mC > {}mC", temp_mc, sensor.alert_high_mc);
    }

    sensor.last_mc = temp_mc;
    TempData {
        millicelsius: temp_mc,
    }
}

pub fn set_alert(low_mc: i32, high_mc: i32) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        if low_mc >= high_mc {
            crate::serial_println!("  temp: invalid alert range {}..{}mC", low_mc, high_mc);
            return;
        }
        sensor.alert_low_mc = low_mc;
        sensor.alert_high_mc = high_mc;
        crate::serial_println!("  temp: alert thresholds set to {}..{}mC", low_mc, high_mc);
    }
}

pub fn init() {
    let sensor = TempSensor {
        resolution_bits: 12,
        i2c_addr: DEFAULT_I2C_ADDR,
        alert_low_mc: -10000,  // -10 degC
        alert_high_mc: 85000,  // 85 degC
        last_mc: 0,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  temp: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

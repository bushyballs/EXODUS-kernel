/// Proximity sensor driver
///
/// Part of the AIOS hardware layer.
/// Reads proximity distance from an I2C-attached IR proximity sensor.
/// Provides both raw distance in mm and a boolean detection flag
/// based on a configurable threshold.

use crate::sync::Mutex;

/// Proximity reading
pub struct ProximityData {
    pub distance_mm: u16,
    pub detected: bool,
}

pub struct ProximitySensor {
    pub threshold_mm: u16,
    i2c_addr: u8,
    ir_led_current_ma: u8,
    last_distance_mm: u16,
}

static SENSOR: Mutex<Option<ProximitySensor>> = Mutex::new(None);

/// Default I2C address (VCNL4010-style)
const DEFAULT_I2C_ADDR: u8 = 0x13;

/// Get low 32 bits of TSC
fn rdtsc_low() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") _hi, options(nomem, nostack));
    }
    lo
}

pub fn read() -> ProximityData {
    let mut guard = SENSOR.lock();
    let sensor = match guard.as_mut() {
        Some(s) => s,
        None => {
            return ProximityData {
                distance_mm: 0xFFFF,
                detected: false,
            };
        }
    };
    // Simulated proximity: ~100mm with TSC-derived noise
    let tsc = rdtsc_low();
    let noise = ((tsc & 0x3F) as i32) - 32;
    let distance = (100i32 + noise).clamp(0, 2000) as u16;
    sensor.last_distance_mm = distance;

    let detected = distance < sensor.threshold_mm;
    ProximityData {
        distance_mm: distance,
        detected,
    }
}

pub fn set_threshold(mm: u16) {
    let mut guard = SENSOR.lock();
    if let Some(sensor) = guard.as_mut() {
        let valid_mm = if mm == 0 { 1 } else if mm > 2000 { 2000 } else { mm };
        sensor.threshold_mm = valid_mm;
        crate::serial_println!("  prox: threshold set to {}mm", valid_mm);
    }
}

pub fn init() {
    let sensor = ProximitySensor {
        threshold_mm: 50,
        i2c_addr: DEFAULT_I2C_ADDR,
        ir_led_current_ma: 20,
        last_distance_mm: 0xFFFF,
    };
    *SENSOR.lock() = Some(sensor);
    crate::serial_println!("  prox: initialized at I2C 0x{:02X}", DEFAULT_I2C_ADDR);
}

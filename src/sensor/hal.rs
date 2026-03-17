/// Sensor Hardware Abstraction Layer
///
/// Part of the AIOS hardware layer.
/// Provides a unified interface for enabling/disabling sensors by index.
/// Each sensor registers a HalSensor descriptor; the HAL tracks enable state
/// and mediates access to the underlying drivers.

use alloc::vec::Vec;
use crate::sync::Mutex;

/// HAL sensor type identifier
#[derive(Clone, Copy, PartialEq)]
pub enum HalSensorType {
    Accelerometer,
    Gyroscope,
    Magnetometer,
    Barometer,
    Light,
    Proximity,
    Temperature,
    Humidity,
    Gps,
}

/// HAL sensor handle
pub struct HalSensor {
    pub sensor_type: HalSensorType,
    pub enabled: bool,
    pub poll_interval_ms: u32,
    pub batch_fifo_depth: u16,
}

static HAL_SENSORS: Mutex<Vec<HalSensor>> = Mutex::new(Vec::new());

pub fn register(sensor: HalSensor) {
    let type_name = sensor_type_name(sensor.sensor_type);
    crate::serial_println!("  hal: registering sensor '{}'", type_name);
    HAL_SENSORS.lock().push(sensor);
}

pub fn enable(idx: usize) {
    let mut guard = HAL_SENSORS.lock();
    if let Some(sensor) = guard.get_mut(idx) {
        if !sensor.enabled {
            sensor.enabled = true;
            let type_name = sensor_type_name(sensor.sensor_type);
            crate::serial_println!("  hal: enabled sensor [{}] '{}'", idx, type_name);
        }
    } else {
        crate::serial_println!("  hal: enable failed, invalid index {}", idx);
    }
}

pub fn disable(idx: usize) {
    let mut guard = HAL_SENSORS.lock();
    if let Some(sensor) = guard.get_mut(idx) {
        if sensor.enabled {
            sensor.enabled = false;
            let type_name = sensor_type_name(sensor.sensor_type);
            crate::serial_println!("  hal: disabled sensor [{}] '{}'", idx, type_name);
        }
    } else {
        crate::serial_println!("  hal: disable failed, invalid index {}", idx);
    }
}

/// Check if a sensor at the given index is currently enabled
pub fn is_enabled(idx: usize) -> bool {
    let guard = HAL_SENSORS.lock();
    guard.get(idx).map_or(false, |s| s.enabled)
}

/// Get the total number of registered HAL sensors
pub fn count() -> usize {
    HAL_SENSORS.lock().len()
}

/// Return a human-readable name for a sensor type
fn sensor_type_name(st: HalSensorType) -> &'static str {
    match st {
        HalSensorType::Accelerometer => "accelerometer",
        HalSensorType::Gyroscope => "gyroscope",
        HalSensorType::Magnetometer => "magnetometer",
        HalSensorType::Barometer => "barometer",
        HalSensorType::Light => "light",
        HalSensorType::Proximity => "proximity",
        HalSensorType::Temperature => "temperature",
        HalSensorType::Humidity => "humidity",
        HalSensorType::Gps => "gps",
    }
}

pub fn init() {
    HAL_SENSORS.lock().clear();
    crate::serial_println!("  hal: sensor HAL initialized");
}

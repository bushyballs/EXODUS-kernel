/// Sensor manager, registration, and polling
///
/// Part of the AIOS hardware layer.
/// Central registry for all sensor descriptors. Sensors register themselves
/// at init time and can be polled by index. The framework tracks sensor
/// metadata (name, type, polling interval) for the upper layers.

use alloc::vec::Vec;
use alloc::string::String;
use crate::sync::Mutex;

/// Sensor type identifiers
pub const SENSOR_TYPE_ACCEL: u32 = 1;
pub const SENSOR_TYPE_GYRO: u32 = 2;
pub const SENSOR_TYPE_MAG: u32 = 3;
pub const SENSOR_TYPE_BARO: u32 = 4;
pub const SENSOR_TYPE_LIGHT: u32 = 5;
pub const SENSOR_TYPE_PROX: u32 = 6;
pub const SENSOR_TYPE_TEMP: u32 = 7;
pub const SENSOR_TYPE_HUMID: u32 = 8;
pub const SENSOR_TYPE_GPS: u32 = 9;

/// Sensor descriptor
pub struct SensorInfo {
    pub name: String,
    pub sensor_type: u32,
    pub poll_interval_ms: u32,
    pub range_max: f32,
    pub resolution: f32,
    pub power_ua: u32,
}

static SENSORS: Mutex<Vec<SensorInfo>> = Mutex::new(Vec::new());

pub fn register(info: SensorInfo) {
    crate::serial_println!("  sensor_fw: registering '{}'", info.name);
    SENSORS.lock().push(info);
}

/// Poll a sensor by registry index. Returns 3-axis data or None if invalid index.
/// This dispatches to the appropriate driver based on sensor_type.
pub fn poll(idx: usize) -> Option<[f32; 3]> {
    let guard = SENSORS.lock();
    let info = guard.get(idx)?;
    let sensor_type = info.sensor_type;
    drop(guard);

    match sensor_type {
        SENSOR_TYPE_ACCEL => {
            let d = super::accelerometer::read();
            Some([d.x as f32, d.y as f32, d.z as f32])
        }
        SENSOR_TYPE_GYRO => {
            let d = super::gyroscope::read();
            Some([d.x as f32, d.y as f32, d.z as f32])
        }
        SENSOR_TYPE_MAG => {
            let d = super::magnetometer::read();
            Some([d.x as f32, d.y as f32, d.z as f32])
        }
        SENSOR_TYPE_BARO => {
            let d = super::barometer::read();
            Some([d.pressure_pa as f32, d.altitude_m, 0.0])
        }
        SENSOR_TYPE_LIGHT => {
            let d = super::light::read();
            Some([d.lux as f32, d.raw_visible as f32, d.raw_ir as f32])
        }
        SENSOR_TYPE_PROX => {
            let d = super::proximity::read();
            Some([d.distance_mm as f32, if d.detected { 1.0 } else { 0.0 }, 0.0])
        }
        SENSOR_TYPE_TEMP => {
            let d = super::temperature::read();
            Some([d.millicelsius as f32, 0.0, 0.0])
        }
        SENSOR_TYPE_HUMID => {
            let d = super::humidity::read();
            Some([d.relative_pct as f32, d.temp_millicelsius as f32, 0.0])
        }
        _ => None,
    }
}

pub fn sensor_count() -> usize {
    SENSORS.lock().len()
}

/// Look up a sensor index by name using FNV-1a hash for fast comparison.
pub fn find_by_name(name: &str) -> Option<usize> {
    let target_hash = fnv1a_hash(name.as_bytes());
    let guard = SENSORS.lock();
    for (i, info) in guard.iter().enumerate() {
        if fnv1a_hash(info.name.as_bytes()) == target_hash && info.name == name {
            return Some(i);
        }
    }
    None
}

fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub fn init() {
    // Clear any stale registrations
    SENSORS.lock().clear();
    crate::serial_println!("  sensor_fw: framework initialized");
}

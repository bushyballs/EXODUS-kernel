/// Sensor framework for Genesis
///
/// Accelerometer, gyroscope, magnetometer, barometer,
/// proximity, light, step counter, and sensor fusion.
///
/// Inspired by: Android SensorManager, iOS CMMotionManager. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Sensor type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    Accelerometer,
    Gyroscope,
    Magnetometer,
    Barometer,
    Proximity,
    AmbientLight,
    Temperature,
    Humidity,
    StepCounter,
    StepDetector,
    Gravity,
    LinearAcceleration,
    RotationVector,
    GameRotation,
    HeartRate,
    Fingerprint,
}

/// Sensor accuracy
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorAccuracy {
    Unreliable,
    Low,
    Medium,
    High,
}

/// A sensor reading (3-axis)
pub struct SensorEvent {
    pub sensor: SensorType,
    pub timestamp: u64,
    pub values: [f32; 4], // x, y, z, w (quaternion uses w)
    pub accuracy: SensorAccuracy,
}

/// Sensor descriptor
pub struct SensorInfo {
    pub sensor_type: SensorType,
    pub name: String,
    pub vendor: String,
    pub max_range: f32,
    pub resolution: f32,
    pub min_delay_us: u32,
    pub max_delay_us: u32,
    pub power_mw: f32,
    pub available: bool,
}

/// Sensor listener registration
pub struct SensorListener {
    pub sensor: SensorType,
    pub delay_us: u32,
    pub app_id: String,
    pub active: bool,
}

/// Sensor manager
pub struct SensorManager {
    pub sensors: Vec<SensorInfo>,
    pub listeners: Vec<SensorListener>,
    pub last_events: Vec<SensorEvent>,
    pub step_count: u64,
}

impl SensorManager {
    const fn new() -> Self {
        SensorManager {
            sensors: Vec::new(),
            listeners: Vec::new(),
            last_events: Vec::new(),
            step_count: 0,
        }
    }

    pub fn register_sensor(&mut self, info: SensorInfo) {
        self.sensors.push(info);
    }

    pub fn register_listener(&mut self, app_id: &str, sensor: SensorType, delay_us: u32) {
        self.listeners.push(SensorListener {
            sensor,
            delay_us,
            app_id: String::from(app_id),
            active: true,
        });
    }

    pub fn unregister_listener(&mut self, app_id: &str, sensor: SensorType) {
        self.listeners
            .retain(|l| !(l.app_id == app_id && l.sensor == sensor));
    }

    pub fn push_event(&mut self, event: SensorEvent) {
        if event.sensor == SensorType::StepDetector {
            self.step_count = self.step_count.saturating_add(1);
        }
        // Update last known event for this sensor type
        if let Some(existing) = self
            .last_events
            .iter_mut()
            .find(|e| e.sensor == event.sensor)
        {
            *existing = event;
        } else {
            self.last_events.push(event);
        }
    }

    pub fn get_last_event(&self, sensor: SensorType) -> Option<&SensorEvent> {
        self.last_events.iter().find(|e| e.sensor == sensor)
    }

    pub fn get_steps(&self) -> u64 {
        self.step_count
    }

    pub fn available_sensors(&self) -> Vec<&SensorInfo> {
        self.sensors.iter().filter(|s| s.available).collect()
    }

    pub fn has_sensor(&self, sensor: SensorType) -> bool {
        self.sensors
            .iter()
            .any(|s| s.sensor_type == sensor && s.available)
    }
}

fn register_builtin_sensors(mgr: &mut SensorManager) {
    let sensors = [
        (SensorType::Accelerometer, "Accelerometer", 39.2, 0.001),
        (SensorType::Gyroscope, "Gyroscope", 34.9, 0.0001),
        (SensorType::Magnetometer, "Magnetometer", 4800.0, 0.1),
        (SensorType::Barometer, "Barometer", 1100.0, 0.01),
        (SensorType::Proximity, "Proximity Sensor", 5.0, 1.0),
        (SensorType::AmbientLight, "Light Sensor", 40000.0, 1.0),
        (SensorType::Temperature, "Temperature", 85.0, 0.1),
        (SensorType::StepCounter, "Step Counter", 1000000.0, 1.0),
        (SensorType::Gravity, "Gravity", 9.81, 0.001),
    ];

    for (stype, name, range, res) in &sensors {
        mgr.register_sensor(SensorInfo {
            sensor_type: *stype,
            name: String::from(*name),
            vendor: String::from("Hoags"),
            max_range: *range as f32,
            resolution: *res as f32,
            min_delay_us: 5000,
            max_delay_us: 200000,
            power_mw: 0.5,
            available: true,
        });
    }
}

static SENSORS: Mutex<SensorManager> = Mutex::new(SensorManager::new());

pub fn init() {
    register_builtin_sensors(&mut SENSORS.lock());
    crate::serial_println!(
        "  [services] Sensor framework initialized ({} sensors)",
        SENSORS.lock().sensors.len()
    );
}

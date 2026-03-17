/// Gyroscope sensor input
///
/// Part of the AIOS hardware layer.
/// Simulates a 3-axis MEMS gyroscope (e.g. BMI160/L3GD20 style).
/// Provides angular velocity readings, configurable range/sample-rate,
/// drift compensation, high-pass filtering, and rotation integration.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// 3-axis angular velocity (mdps -- milli-degrees per second)
pub struct GyroReading {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Drift compensation offsets (mdps), computed during stationary calibration
#[derive(Clone, Copy)]
struct DriftOffsets {
    x: i32,
    y: i32,
    z: i32,
}

/// High-pass filter to remove DC bias / slow drift
#[derive(Clone, Copy)]
struct HighPassFilter {
    /// Filter coefficient (0..256, higher = more filtering)
    alpha: u16,
    prev_x: i32,
    prev_y: i32,
    prev_z: i32,
    out_x: i32,
    out_y: i32,
    out_z: i32,
    initialized: bool,
}

impl HighPassFilter {
    fn new(alpha: u16) -> Self {
        HighPassFilter {
            alpha: alpha.min(256),
            prev_x: 0,
            prev_y: 0,
            prev_z: 0,
            out_x: 0,
            out_y: 0,
            out_z: 0,
            initialized: false,
        }
    }

    /// High-pass: output = alpha/256 * (prev_output + input - prev_input)
    fn apply(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        if !self.initialized {
            self.prev_x = x;
            self.prev_y = y;
            self.prev_z = z;
            self.out_x = 0;
            self.out_y = 0;
            self.out_z = 0;
            self.initialized = true;
            return (0, 0, 0);
        }
        let a = self.alpha as i64;
        self.out_x = ((a * (self.out_x as i64 + x as i64 - self.prev_x as i64)) / 256) as i32;
        self.out_y = ((a * (self.out_y as i64 + y as i64 - self.prev_y as i64)) / 256) as i32;
        self.out_z = ((a * (self.out_z as i64 + z as i64 - self.prev_z as i64)) / 256) as i32;
        self.prev_x = x;
        self.prev_y = y;
        self.prev_z = z;
        (self.out_x, self.out_y, self.out_z)
    }

    fn reset(&mut self) {
        self.initialized = false;
        self.prev_x = 0;
        self.prev_y = 0;
        self.prev_z = 0;
        self.out_x = 0;
        self.out_y = 0;
        self.out_z = 0;
    }
}

/// Integrated rotation angles in milli-degrees
#[derive(Clone, Copy)]
struct IntegratedAngles {
    roll: i64,  // rotation about X axis
    pitch: i64, // rotation about Y axis
    yaw: i64,   // rotation about Z axis
}

/// Calibration state
#[derive(Clone, Copy, PartialEq)]
pub enum CalibrationState {
    Uncalibrated,
    Collecting,
    Calibrated,
}

/// Stationary calibration collector
#[derive(Clone, Copy)]
struct CalibrationCollector {
    sum_x: i64,
    sum_y: i64,
    sum_z: i64,
    count: u32,
}

impl CalibrationCollector {
    fn new() -> Self {
        CalibrationCollector {
            sum_x: 0,
            sum_y: 0,
            sum_z: 0,
            count: 0,
        }
    }

    fn add_sample(&mut self, x: i32, y: i32, z: i32) {
        self.sum_x += x as i64;
        self.sum_y += y as i64;
        self.sum_z += z as i64;
        self.count = self.count.saturating_add(1);
    }

    /// Compute average drift offset
    fn compute_offsets(&self) -> DriftOffsets {
        if self.count == 0 {
            return DriftOffsets { x: 0, y: 0, z: 0 };
        }
        let n = self.count as i64;
        DriftOffsets {
            x: (self.sum_x / n) as i32,
            y: (self.sum_y / n) as i32,
            z: (self.sum_z / n) as i32,
        }
    }
}

pub struct Gyroscope {
    pub range_dps: u16,
    pub sample_rate_hz: u16,
    /// Drift compensation offsets
    drift: DriftOffsets,
    /// High-pass filter
    hpf: HighPassFilter,
    /// Raw readings (before compensation)
    raw_x: i32,
    raw_y: i32,
    raw_z: i32,
    /// Compensated/filtered readings
    filtered_x: i32,
    filtered_y: i32,
    filtered_z: i32,
    /// Integrated rotation angles (milli-degrees)
    angles: IntegratedAngles,
    /// Sample interval in microseconds (for integration)
    sample_interval_us: u32,
    /// Calibration state
    cal_state: CalibrationState,
    /// Calibration collector
    cal_collector: CalibrationCollector,
    /// Sample counter
    sample_count: u64,
    /// Whether high-pass filter is enabled
    hpf_enabled: bool,
    /// Temperature compensation coefficient (x1000)
    temp_coeff: i32,
    /// Current temperature in centi-degrees C
    temperature_cdeg: i32,
    /// Whether device is operational
    operational: bool,
}

static DEVICE: Mutex<Option<Gyroscope>> = Mutex::new(None);

impl Gyroscope {
    fn new() -> Self {
        Gyroscope {
            range_dps: 250,
            sample_rate_hz: 100,
            drift: DriftOffsets { x: 0, y: 0, z: 0 },
            hpf: HighPassFilter::new(240),
            raw_x: 0,
            raw_y: 0,
            raw_z: 0,
            filtered_x: 0,
            filtered_y: 0,
            filtered_z: 0,
            angles: IntegratedAngles {
                roll: 0,
                pitch: 0,
                yaw: 0,
            },
            sample_interval_us: 10_000, // 100 Hz = 10ms
            cal_state: CalibrationState::Uncalibrated,
            cal_collector: CalibrationCollector::new(),
            sample_count: 0,
            hpf_enabled: true,
            temp_coeff: 0,
            temperature_cdeg: 2500, // 25.00 C default
            operational: true,
        }
    }

    /// Set measurement range (125, 250, 500, 1000, 2000 dps)
    fn set_range(&mut self, dps: u16) {
        let valid = match dps {
            0..=125 => 125,
            126..=250 => 250,
            251..=500 => 500,
            501..=1000 => 1000,
            _ => 2000,
        };
        self.range_dps = valid;
        self.hpf.reset();
        serial_println!("    [gyro] range set to +/-{} dps", valid);
    }

    /// Set sample rate (Hz) and update integration interval
    fn set_sample_rate(&mut self, hz: u16) {
        let valid = match hz {
            0..=25 => 25,
            26..=50 => 50,
            51..=100 => 100,
            101..=200 => 200,
            201..=400 => 400,
            _ => 800,
        };
        self.sample_rate_hz = valid;
        self.sample_interval_us = 1_000_000 / valid as u32;
        serial_println!(
            "    [gyro] sample rate set to {} Hz (interval {} us)",
            valid,
            self.sample_interval_us
        );
    }

    /// Process a new raw gyroscope sample (in mdps)
    fn process_sample(&mut self, raw_x: i32, raw_y: i32, raw_z: i32) {
        self.raw_x = raw_x;
        self.raw_y = raw_y;
        self.raw_z = raw_z;

        // If calibrating, collect sample
        if self.cal_state == CalibrationState::Collecting {
            self.cal_collector.add_sample(raw_x, raw_y, raw_z);
        }

        // Apply drift compensation
        let dx = raw_x - self.drift.x;
        let dy = raw_y - self.drift.y;
        let dz = raw_z - self.drift.z;

        // Apply temperature compensation if configured
        let tx = if self.temp_coeff != 0 {
            let temp_delta = self.temperature_cdeg - 2500; // delta from 25C
            dx - (self.temp_coeff as i64 * temp_delta as i64 / 1000) as i32
        } else {
            dx
        };
        let ty = if self.temp_coeff != 0 {
            let temp_delta = self.temperature_cdeg - 2500;
            dy - (self.temp_coeff as i64 * temp_delta as i64 / 1000) as i32
        } else {
            dy
        };
        let tz = if self.temp_coeff != 0 {
            let temp_delta = self.temperature_cdeg - 2500;
            dz - (self.temp_coeff as i64 * temp_delta as i64 / 1000) as i32
        } else {
            dz
        };

        // Clamp to range
        let max_mdps = self.range_dps as i32 * 1000;
        let cx = clamp(tx, -max_mdps, max_mdps);
        let cy = clamp(ty, -max_mdps, max_mdps);
        let cz = clamp(tz, -max_mdps, max_mdps);

        // Apply high-pass filter if enabled
        let (fx, fy, fz) = if self.hpf_enabled {
            self.hpf.apply(cx, cy, cz)
        } else {
            (cx, cy, cz)
        };

        self.filtered_x = fx;
        self.filtered_y = fy;
        self.filtered_z = fz;

        // Integrate angular velocity to get rotation angles
        // angle_change (mdeg) = angular_velocity (mdps) * dt (s)
        // = angular_velocity (mdps) * sample_interval_us / 1_000_000
        let dt_us = self.sample_interval_us as i64;
        self.angles.roll += (fx as i64 * dt_us) / 1_000_000;
        self.angles.pitch += (fy as i64 * dt_us) / 1_000_000;
        self.angles.yaw += (fz as i64 * dt_us) / 1_000_000;

        self.sample_count = self.sample_count.saturating_add(1);
    }

    /// Read current filtered angular velocity
    fn read(&self) -> GyroReading {
        GyroReading {
            x: self.filtered_x,
            y: self.filtered_y,
            z: self.filtered_z,
        }
    }

    /// Get integrated rotation angles in milli-degrees
    fn get_angles(&self) -> (i64, i64, i64) {
        (self.angles.roll, self.angles.pitch, self.angles.yaw)
    }

    /// Reset integrated angles to zero
    fn reset_angles(&mut self) {
        self.angles = IntegratedAngles {
            roll: 0,
            pitch: 0,
            yaw: 0,
        };
    }

    /// Start stationary calibration (device must be still)
    fn start_calibration(&mut self) {
        self.cal_state = CalibrationState::Collecting;
        self.cal_collector = CalibrationCollector::new();
        serial_println!("    [gyro] calibration started - keep device stationary");
    }

    /// Finish calibration and compute drift offsets
    fn finish_calibration(&mut self) -> bool {
        if self.cal_collector.count < 100 {
            serial_println!(
                "    [gyro] calibration failed: need 100+ samples, got {}",
                self.cal_collector.count
            );
            return false;
        }
        self.drift = self.cal_collector.compute_offsets();
        self.cal_state = CalibrationState::Calibrated;
        self.hpf.reset();
        serial_println!(
            "    [gyro] calibration complete: drift=({},{},{}) mdps",
            self.drift.x,
            self.drift.y,
            self.drift.z
        );
        true
    }

    /// Set temperature for compensation
    fn set_temperature(&mut self, cdeg: i32) {
        self.temperature_cdeg = cdeg;
    }

    /// Enable or disable the high-pass filter
    fn set_hpf_enabled(&mut self, enabled: bool) {
        self.hpf_enabled = enabled;
        if !enabled {
            self.hpf.reset();
        }
    }

    /// Perform self-test: gyro should read near-zero when stationary
    fn self_test(&self) -> bool {
        let threshold = 5000; // 5 dps in mdps
        abs_i32(self.filtered_x) < threshold
            && abs_i32(self.filtered_y) < threshold
            && abs_i32(self.filtered_z) < threshold
    }

    fn sample_count(&self) -> u64 {
        self.sample_count
    }
}

fn abs_i32(v: i32) -> i32 {
    if v < 0 {
        -v
    } else {
        v
    }
}

fn clamp(val: i32, min: i32, max: i32) -> i32 {
    if val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}

/// Read the current gyroscope values (public API)
pub fn read() -> GyroReading {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.read(),
        None => {
            serial_println!("    [gyro] device not initialized, returning zero");
            GyroReading { x: 0, y: 0, z: 0 }
        }
    }
}

/// Set the gyroscope measurement range in dps (public API)
pub fn set_range(dps: u16) {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.set_range(dps);
    } else {
        serial_println!("    [gyro] cannot set range: device not initialized");
    }
}

/// Submit a raw gyroscope sample for processing
pub fn submit_sample(raw_x: i32, raw_y: i32, raw_z: i32) {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.process_sample(raw_x, raw_y, raw_z);
    }
}

/// Get integrated rotation angles (roll, pitch, yaw) in milli-degrees
pub fn get_angles() -> (i64, i64, i64) {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.get_angles(),
        None => (0, 0, 0),
    }
}

/// Reset integrated angles to zero
pub fn reset_angles() {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.reset_angles();
    }
}

/// Start stationary drift calibration
pub fn start_calibration() {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.start_calibration();
    }
}

/// Finish calibration
pub fn finish_calibration() -> bool {
    let mut guard = DEVICE.lock();
    match guard.as_mut() {
        Some(dev) => dev.finish_calibration(),
        None => false,
    }
}

/// Initialize the gyroscope subsystem
pub fn init() {
    let mut guard = DEVICE.lock();
    let mut gyro = Gyroscope::new();
    // Simulate initial stationary sample
    gyro.process_sample(0, 0, 0);
    *guard = Some(gyro);
    serial_println!(
        "    [gyro] gyroscope initialized: +/-250 dps, 100 Hz, HPF active, angle integration"
    );
}

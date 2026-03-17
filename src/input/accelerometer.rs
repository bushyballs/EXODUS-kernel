/// Accelerometer sensor input
///
/// Part of the AIOS hardware layer.
/// Simulates a 3-axis MEMS accelerometer (e.g. LIS3DH/ADXL345 style).
/// Manages calibration offsets, configurable range/sample-rate,
/// low-pass filtering of readings, and tap/freefall detection.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// 3-axis acceleration reading (mg)
pub struct AccelReading {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Calibration offsets per axis (mg)
#[derive(Clone, Copy)]
struct CalibrationOffsets {
    x: i32,
    y: i32,
    z: i32,
}

/// Tap detection state
#[derive(Clone, Copy, PartialEq)]
pub enum TapState {
    None,
    SingleTap,
    DoubleTap,
}

/// Freefall detection state
#[derive(Clone, Copy, PartialEq)]
pub enum FreefallState {
    Grounded,
    Freefall,
}

/// Low-pass filter for smoothing readings
#[derive(Clone, Copy)]
struct LowPassFilter {
    /// Filter coefficient (0..256 where 256 = no filtering)
    alpha: u16,
    /// Previous filtered output per axis
    prev_x: i32,
    prev_y: i32,
    prev_z: i32,
    /// Whether the filter has been primed with a first sample
    initialized: bool,
}

impl LowPassFilter {
    fn new(alpha: u16) -> Self {
        LowPassFilter {
            alpha: if alpha > 256 { 256 } else { alpha },
            prev_x: 0,
            prev_y: 0,
            prev_z: 0,
            initialized: false,
        }
    }

    /// Apply the low-pass filter: output = alpha/256 * input + (256-alpha)/256 * prev
    fn apply(&mut self, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        if !self.initialized {
            self.prev_x = x;
            self.prev_y = y;
            self.prev_z = z;
            self.initialized = true;
            return (x, y, z);
        }
        let a = self.alpha as i64;
        let inv = 256 - a;
        let fx = ((a * x as i64 + inv * self.prev_x as i64) / 256) as i32;
        let fy = ((a * y as i64 + inv * self.prev_y as i64) / 256) as i32;
        let fz = ((a * z as i64 + inv * self.prev_z as i64) / 256) as i32;
        self.prev_x = fx;
        self.prev_y = fy;
        self.prev_z = fz;
        (fx, fy, fz)
    }

    fn reset(&mut self) {
        self.initialized = false;
        self.prev_x = 0;
        self.prev_y = 0;
        self.prev_z = 0;
    }
}

pub struct Accelerometer {
    pub range_g: u8,
    pub sample_rate_hz: u16,
    /// Calibration offsets subtracted from raw readings
    calibration: CalibrationOffsets,
    /// Low-pass filter for smoothing
    filter: LowPassFilter,
    /// Last raw reading (before calibration/filter)
    raw_x: i32,
    raw_y: i32,
    raw_z: i32,
    /// Last filtered reading
    filtered_x: i32,
    filtered_y: i32,
    filtered_z: i32,
    /// Tap detection threshold (mg)
    tap_threshold: i32,
    /// Previous magnitude for tap detection
    prev_magnitude: i32,
    /// Freefall threshold (mg) -- below this total magnitude = freefall
    freefall_threshold: i32,
    /// Current tap state
    tap_state: TapState,
    /// Current freefall state
    freefall_state: FreefallState,
    /// Sample counter for statistics
    sample_count: u64,
    /// Whether device is initialized and operational
    operational: bool,
}

static DEVICE: Mutex<Option<Accelerometer>> = Mutex::new(None);

impl Accelerometer {
    /// Create a new accelerometer with default 2g range, 100 Hz sample rate
    fn new() -> Self {
        Accelerometer {
            range_g: 2,
            sample_rate_hz: 100,
            calibration: CalibrationOffsets { x: 0, y: 0, z: 0 },
            filter: LowPassFilter::new(64), // moderate smoothing
            raw_x: 0,
            raw_y: 0,
            raw_z: 1000, // 1g on Z (resting on table)
            filtered_x: 0,
            filtered_y: 0,
            filtered_z: 1000,
            tap_threshold: 1500, // 1.5g tap threshold
            prev_magnitude: 1000,
            freefall_threshold: 200, // below 0.2g total = freefall
            tap_state: TapState::None,
            freefall_state: FreefallState::Grounded,
            sample_count: 0,
            operational: true,
        }
    }

    /// Set the measurement range in g (2, 4, 8, or 16)
    fn set_range(&mut self, g: u8) {
        let valid_range = match g {
            0..=2 => 2,
            3..=4 => 4,
            5..=8 => 8,
            _ => 16,
        };
        self.range_g = valid_range;
        // Reset filter when range changes since scale changes
        self.filter.reset();
        serial_println!("    [accel] range set to +/-{}g", valid_range);
    }

    /// Set sample rate in Hz
    fn set_sample_rate(&mut self, hz: u16) {
        let valid_rate = match hz {
            0..=12 => 10,
            13..=25 => 25,
            26..=50 => 50,
            51..=100 => 100,
            101..=200 => 200,
            201..=400 => 400,
            _ => 800,
        };
        self.sample_rate_hz = valid_rate;
        serial_println!("    [accel] sample rate set to {} Hz", valid_rate);
    }

    /// Set calibration offsets (typically determined during factory calibration)
    fn set_calibration(&mut self, x: i32, y: i32, z: i32) {
        self.calibration = CalibrationOffsets { x, y, z };
        serial_println!(
            "    [accel] calibration offsets set: x={} y={} z={} mg",
            x,
            y,
            z
        );
    }

    /// Process a new raw sample from the hardware (or simulated)
    fn process_sample(&mut self, raw_x: i32, raw_y: i32, raw_z: i32) {
        self.raw_x = raw_x;
        self.raw_y = raw_y;
        self.raw_z = raw_z;

        // Apply calibration offsets
        let cal_x = raw_x - self.calibration.x;
        let cal_y = raw_y - self.calibration.y;
        let cal_z = raw_z - self.calibration.z;

        // Clamp to range
        let max_mg = (self.range_g as i32) * 1000;
        let clamped_x = clamp(cal_x, -max_mg, max_mg);
        let clamped_y = clamp(cal_y, -max_mg, max_mg);
        let clamped_z = clamp(cal_z, -max_mg, max_mg);

        // Apply low-pass filter
        let (fx, fy, fz) = self.filter.apply(clamped_x, clamped_y, clamped_z);
        self.filtered_x = fx;
        self.filtered_y = fy;
        self.filtered_z = fz;

        // Compute magnitude for tap/freefall detection
        // Using approximate magnitude: |x| + |y| + |z| (avoids sqrt)
        let magnitude = abs_i32(fx) + abs_i32(fy) + abs_i32(fz);

        // Tap detection: sudden spike in magnitude
        let delta = abs_i32(magnitude - self.prev_magnitude);
        if delta > self.tap_threshold {
            self.tap_state = TapState::SingleTap;
        } else {
            self.tap_state = TapState::None;
        }

        // Freefall detection: total magnitude near zero
        if magnitude < self.freefall_threshold {
            self.freefall_state = FreefallState::Freefall;
        } else {
            self.freefall_state = FreefallState::Grounded;
        }

        self.prev_magnitude = magnitude;
        self.sample_count = self.sample_count.saturating_add(1);
    }

    /// Read the current filtered acceleration
    fn read(&self) -> AccelReading {
        AccelReading {
            x: self.filtered_x,
            y: self.filtered_y,
            z: self.filtered_z,
        }
    }

    /// Get the current tap detection state
    fn tap_detected(&self) -> TapState {
        self.tap_state
    }

    /// Get the current freefall state
    fn freefall_detected(&self) -> FreefallState {
        self.freefall_state
    }

    /// Get total sample count since init
    fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// Set the low-pass filter coefficient (0-256)
    fn set_filter_alpha(&mut self, alpha: u16) {
        self.filter = LowPassFilter::new(alpha);
        serial_println!("    [accel] filter alpha set to {}", alpha);
    }

    /// Set tap detection threshold in mg
    fn set_tap_threshold(&mut self, mg: i32) {
        self.tap_threshold = mg;
    }

    /// Perform self-test by checking Z-axis reads approximately 1g
    fn self_test(&self) -> bool {
        let z = abs_i32(self.filtered_z);
        // Expect Z around 1000mg when at rest, allow +/- 300mg
        z > 700 && z < 1300
    }
}

/// Absolute value for i32 without std
fn abs_i32(v: i32) -> i32 {
    if v < 0 {
        -v
    } else {
        v
    }
}

/// Clamp value to [min, max]
fn clamp(val: i32, min: i32, max: i32) -> i32 {
    if val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}

/// Read the current accelerometer values (public API)
pub fn read() -> AccelReading {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.read(),
        None => {
            serial_println!("    [accel] device not initialized, returning zero");
            AccelReading { x: 0, y: 0, z: 0 }
        }
    }
}

/// Set the accelerometer measurement range
pub fn set_range(g: u8) {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.set_range(g);
    } else {
        serial_println!("    [accel] cannot set range: device not initialized");
    }
}

/// Submit a raw sample for processing (called by sensor hub / I2C driver)
pub fn submit_sample(raw_x: i32, raw_y: i32, raw_z: i32) {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.process_sample(raw_x, raw_y, raw_z);
    }
}

/// Get tap detection state
pub fn tap_state() -> TapState {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.tap_detected(),
        None => TapState::None,
    }
}

/// Get freefall detection state
pub fn freefall_state() -> FreefallState {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.freefall_detected(),
        None => FreefallState::Grounded,
    }
}

/// Initialize the accelerometer subsystem
pub fn init() {
    let mut guard = DEVICE.lock();
    let mut accel = Accelerometer::new();
    // Simulate initial resting sample (1g on Z axis)
    accel.process_sample(0, 0, 1000);
    *guard = Some(accel);
    serial_println!("    [accel] accelerometer initialized: +/-2g, 100 Hz, LP filter active");
}

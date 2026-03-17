/// Magnetometer/compass sensor input
///
/// Part of the AIOS hardware layer.
/// Simulates a 3-axis magnetometer (e.g. HMC5883L/QMC5883L style).
/// Provides magnetic field readings, heading computation,
/// hard-iron / soft-iron calibration, and tilt compensation.
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

/// 3-axis magnetic field reading (uT -- microtesla)
pub struct CompassReading {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

/// Hard-iron calibration offsets (uT)
#[derive(Clone, Copy)]
struct HardIronOffsets {
    x: i32,
    y: i32,
    z: i32,
}

/// Soft-iron scale factors (x100 for integer math, 100 = 1.0)
#[derive(Clone, Copy)]
struct SoftIronScale {
    x: i32,
    y: i32,
    z: i32,
}

/// Compass calibration state
#[derive(Clone, Copy, PartialEq)]
pub enum CalibrationState {
    Uncalibrated,
    Collecting,
    Calibrated,
}

/// Running min/max tracker for calibration
#[derive(Clone, Copy)]
struct CalibrationCollector {
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    min_z: i32,
    max_z: i32,
    sample_count: u32,
}

impl CalibrationCollector {
    fn new() -> Self {
        CalibrationCollector {
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_y: i32::MAX,
            max_y: i32::MIN,
            min_z: i32::MAX,
            max_z: i32::MIN,
            sample_count: 0,
        }
    }

    fn add_sample(&mut self, x: i32, y: i32, z: i32) {
        if x < self.min_x {
            self.min_x = x;
        }
        if x > self.max_x {
            self.max_x = x;
        }
        if y < self.min_y {
            self.min_y = y;
        }
        if y > self.max_y {
            self.max_y = y;
        }
        if z < self.min_z {
            self.min_z = z;
        }
        if z > self.max_z {
            self.max_z = z;
        }
        self.sample_count = self.sample_count.saturating_add(1);
    }

    /// Compute hard-iron offsets (midpoint of min/max)
    fn compute_hard_iron(&self) -> HardIronOffsets {
        HardIronOffsets {
            x: (self.min_x + self.max_x) / 2,
            y: (self.min_y + self.max_y) / 2,
            z: (self.min_z + self.max_z) / 2,
        }
    }

    /// Compute soft-iron scale factors (normalize axis ranges)
    fn compute_soft_iron(&self) -> SoftIronScale {
        let range_x = (self.max_x - self.min_x).max(1);
        let range_y = (self.max_y - self.min_y).max(1);
        let range_z = (self.max_z - self.min_z).max(1);
        // Average range
        let avg_range = (range_x + range_y + range_z) / 3;
        SoftIronScale {
            x: (avg_range * 100) / range_x,
            y: (avg_range * 100) / range_y,
            z: (avg_range * 100) / range_z,
        }
    }
}

/// Moving average filter for smoothing readings
#[derive(Clone, Copy)]
struct MovingAverage {
    buffer_x: [i32; 8],
    buffer_y: [i32; 8],
    buffer_z: [i32; 8],
    index: usize,
    count: usize,
}

impl MovingAverage {
    fn new() -> Self {
        MovingAverage {
            buffer_x: [0; 8],
            buffer_y: [0; 8],
            buffer_z: [0; 8],
            index: 0,
            count: 0,
        }
    }

    fn add(&mut self, x: i32, y: i32, z: i32) {
        self.buffer_x[self.index] = x;
        self.buffer_y[self.index] = y;
        self.buffer_z[self.index] = z;
        self.index = (self.index + 1) % 8;
        if self.count < 8 {
            self.count += 1;
        }
    }

    fn average(&self) -> (i32, i32, i32) {
        if self.count == 0 {
            return (0, 0, 0);
        }
        let n = self.count as i32;
        let mut sx: i64 = 0;
        let mut sy: i64 = 0;
        let mut sz: i64 = 0;
        for i in 0..self.count {
            sx += self.buffer_x[i] as i64;
            sy += self.buffer_y[i] as i64;
            sz += self.buffer_z[i] as i64;
        }
        (
            (sx / n as i64) as i32,
            (sy / n as i64) as i32,
            (sz / n as i64) as i32,
        )
    }

    fn reset(&mut self) {
        self.buffer_x = [0; 8];
        self.buffer_y = [0; 8];
        self.buffer_z = [0; 8];
        self.index = 0;
        self.count = 0;
    }
}

pub struct Compass {
    pub range_ut: u16,
    pub sample_rate_hz: u16,
    /// Hard-iron calibration offsets
    hard_iron: HardIronOffsets,
    /// Soft-iron scale correction
    soft_iron: SoftIronScale,
    /// Calibration state
    cal_state: CalibrationState,
    /// Calibration data collector
    cal_collector: CalibrationCollector,
    /// Moving average filter
    avg_filter: MovingAverage,
    /// Last calibrated reading
    cal_x: i32,
    cal_y: i32,
    cal_z: i32,
    /// Declination angle offset in tenths of a degree
    declination_tenths: i32,
    /// Total samples processed
    sample_count: u64,
    /// Device operational
    operational: bool,
}

static DEVICE: Mutex<Option<Compass>> = Mutex::new(None);

impl Compass {
    fn new() -> Self {
        Compass {
            range_ut: 800, // +/- 800 uT typical
            sample_rate_hz: 75,
            hard_iron: HardIronOffsets { x: 0, y: 0, z: 0 },
            soft_iron: SoftIronScale {
                x: 100,
                y: 100,
                z: 100,
            },
            cal_state: CalibrationState::Uncalibrated,
            cal_collector: CalibrationCollector::new(),
            avg_filter: MovingAverage::new(),
            cal_x: 0,
            cal_y: 20,  // Simulated earth field ~20 uT on Y
            cal_z: -40, // Simulated vertical component
            declination_tenths: 0,
            sample_count: 0,
            operational: true,
        }
    }

    /// Process a raw magnetometer sample
    fn process_sample(&mut self, raw_x: i32, raw_y: i32, raw_z: i32) {
        // If calibrating, collect samples
        if self.cal_state == CalibrationState::Collecting {
            self.cal_collector.add_sample(raw_x, raw_y, raw_z);
        }

        // Apply hard-iron correction (subtract offsets)
        let hx = raw_x - self.hard_iron.x;
        let hy = raw_y - self.hard_iron.y;
        let hz = raw_z - self.hard_iron.z;

        // Apply soft-iron correction (scale factors)
        let sx = (hx as i64 * self.soft_iron.x as i64 / 100) as i32;
        let sy = (hy as i64 * self.soft_iron.y as i64 / 100) as i32;
        let sz = (hz as i64 * self.soft_iron.z as i64 / 100) as i32;

        // Clamp to range
        let max = self.range_ut as i32;
        let cx = clamp(sx, -max, max);
        let cy = clamp(sy, -max, max);
        let cz = clamp(sz, -max, max);

        // Apply moving average filter
        self.avg_filter.add(cx, cy, cz);
        let (fx, fy, fz) = self.avg_filter.average();

        self.cal_x = fx;
        self.cal_y = fy;
        self.cal_z = fz;
        self.sample_count = self.sample_count.saturating_add(1);
    }

    /// Start calibration collection
    fn start_calibration(&mut self) {
        self.cal_state = CalibrationState::Collecting;
        self.cal_collector = CalibrationCollector::new();
        serial_println!("    [compass] calibration started - rotate device in all directions");
    }

    /// Finish calibration and compute offsets
    fn finish_calibration(&mut self) -> bool {
        if self.cal_collector.sample_count < 50 {
            serial_println!(
                "    [compass] calibration failed: need at least 50 samples, got {}",
                self.cal_collector.sample_count
            );
            return false;
        }
        self.hard_iron = self.cal_collector.compute_hard_iron();
        self.soft_iron = self.cal_collector.compute_soft_iron();
        self.cal_state = CalibrationState::Calibrated;
        self.avg_filter.reset();
        serial_println!(
            "    [compass] calibration complete: hard_iron=({},{},{}), soft_iron=({},{},{})",
            self.hard_iron.x,
            self.hard_iron.y,
            self.hard_iron.z,
            self.soft_iron.x,
            self.soft_iron.y,
            self.soft_iron.z
        );
        true
    }

    /// Read the calibrated magnetic field
    fn read(&self) -> CompassReading {
        CompassReading {
            x: self.cal_x,
            y: self.cal_y,
            z: self.cal_z,
        }
    }

    /// Compute heading in degrees (0-359) from X/Y magnetic components
    /// Uses integer atan2 approximation
    fn heading_degrees(&self) -> f32 {
        let x = self.cal_x as f64;
        let y = self.cal_y as f64;

        // atan2 approximation using CORDIC-like integer math
        // We compute heading in tenths of degrees for precision
        let heading_tenths = atan2_approx_tenths(y as i32, x as i32);

        // Apply declination offset
        let adjusted = heading_tenths + self.declination_tenths;

        // Normalize to 0-3600 range (tenths of degrees)
        let normalized = if adjusted < 0 {
            adjusted + 3600
        } else if adjusted >= 3600 {
            adjusted - 3600
        } else {
            adjusted
        };

        normalized as f32 / 10.0
    }

    /// Set magnetic declination (tenths of degrees)
    fn set_declination(&mut self, tenths: i32) {
        self.declination_tenths = tenths;
        serial_println!(
            "    [compass] declination set to {}.{} degrees",
            tenths / 10,
            (tenths % 10).abs()
        );
    }

    /// Get calibration state
    fn calibration_state(&self) -> CalibrationState {
        self.cal_state
    }

    /// Magnetic field magnitude (approximate)
    fn magnitude(&self) -> i32 {
        // Approximate |v| = max(|x|,|y|,|z|) + min(|x|,|y|,|z|)/2
        let ax = abs_i32(self.cal_x);
        let ay = abs_i32(self.cal_y);
        let az = abs_i32(self.cal_z);
        let max_val = ax.max(ay).max(az);
        let min_val = ax.min(ay).min(az);
        max_val + min_val / 2
    }
}

/// atan2 approximation returning tenths of degrees (0-3599)
/// Uses octant-based polynomial approximation
fn atan2_approx_tenths(y: i32, x: i32) -> i32 {
    if x == 0 && y == 0 {
        return 0;
    }

    let ax = abs_i32(x) as i64;
    let ay = abs_i32(y) as i64;

    // Compute angle in first octant (0-450 tenths = 0-45 degrees)
    // atan(y/x) ~= (45 * y/x) for small angles, refined with correction
    let (numer, denom) = if ax >= ay { (ay, ax) } else { (ax, ay) };
    let denom = if denom == 0 { 1 } else { denom };

    // ratio = numer/denom in range [0, 1]
    // atan(r) approx = 450 * r / (1 + 0.28 * r^2)  (in tenths of degrees)
    // Simplified: 450 * numer / denom
    let ratio_x1000 = (numer * 1000) / denom;
    let r_sq = (ratio_x1000 * ratio_x1000) / 1000;
    let angle = (450 * ratio_x1000) / (1000 + (280 * r_sq) / 1000);

    // Map to correct quadrant
    let angle = if ax < ay { 900 - angle } else { angle };
    let angle = if x < 0 { 1800 - angle } else { angle };
    let angle = if y < 0 { 3600 - angle } else { angle };

    // Normalize
    let angle = if angle < 0 { angle + 3600 } else { angle };
    let angle = if angle >= 3600 { angle - 3600 } else { angle };

    angle as i32
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

/// Read the current compass values (public API)
pub fn read() -> CompassReading {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.read(),
        None => {
            serial_println!("    [compass] device not initialized, returning zero");
            CompassReading { x: 0, y: 0, z: 0 }
        }
    }
}

/// Compute heading in degrees (public API)
pub fn heading_degrees() -> f32 {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.heading_degrees(),
        None => {
            serial_println!("    [compass] device not initialized, returning 0.0");
            0.0
        }
    }
}

/// Submit a raw magnetometer sample
pub fn submit_sample(raw_x: i32, raw_y: i32, raw_z: i32) {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.process_sample(raw_x, raw_y, raw_z);
    }
}

/// Start calibration process
pub fn start_calibration() {
    let mut guard = DEVICE.lock();
    if let Some(dev) = guard.as_mut() {
        dev.start_calibration();
    }
}

/// Finish calibration and apply offsets
pub fn finish_calibration() -> bool {
    let mut guard = DEVICE.lock();
    match guard.as_mut() {
        Some(dev) => dev.finish_calibration(),
        None => false,
    }
}

/// Get the magnetic field magnitude (approximate, in uT)
pub fn magnitude() -> i32 {
    let guard = DEVICE.lock();
    match guard.as_ref() {
        Some(dev) => dev.magnitude(),
        None => 0,
    }
}

/// Initialize the compass subsystem
pub fn init() {
    let mut guard = DEVICE.lock();
    let mut compass = Compass::new();
    // Simulate an initial earth-field reading
    compass.process_sample(5, 20, -40);
    *guard = Some(compass);
    serial_println!("    [compass] magnetometer initialized: +/-800 uT, 75 Hz, atan2 heading");
}

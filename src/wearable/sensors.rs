use crate::serial_println;
/// Wearable sensor subsystem for Genesis
///
/// Provides a unified interface for reading wearable hardware sensors:
///   - Heart rate monitor (optical PPG)
///   - Step counter (accelerometer-derived)
///   - Gyroscope (3-axis orientation)
///   - GPS (fix, coordinates, altitude, speed)
///
/// Each sensor is modelled as a static driver instance protected by a Mutex.
/// Values are stored in fixed-point integers to remain `no_std` / no-float.
///
/// Coordinates use fixed-point with 1e-7 degree resolution (WGS-84 millionths
/// of a degree in i32: ±2147483647 covers ±214.7°, sufficient for lat/lon).
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;

// ============================================================================
// Heart-rate sensor
// ============================================================================

/// Heart rate sensor state
pub struct HeartRateSensor {
    /// Latest reading in beats per minute (0 = no reading)
    pub bpm: u16,
    /// Confidence percentage (0-100)
    pub confidence: u8,
    /// Timestamp of last valid reading (kernel uptime ms)
    pub last_update_ms: u64,
    /// Whether sensor hardware is present and initialised
    pub present: bool,
}

impl HeartRateSensor {
    const fn new() -> Self {
        HeartRateSensor {
            bpm: 0,
            confidence: 0,
            last_update_ms: 0,
            present: false,
        }
    }

    /// Update with a new raw PPG sample.
    ///
    /// `raw_bpm` — hardware-reported BPM
    /// `confidence` — hardware-reported confidence (0-100)
    /// `timestamp_ms` — current uptime in milliseconds
    pub fn update(&mut self, raw_bpm: u16, confidence: u8, timestamp_ms: u64) {
        self.bpm = raw_bpm;
        self.confidence = confidence.min(100);
        self.last_update_ms = timestamp_ms;
    }

    /// Returns `true` if the reading is fresh (< 5 s old) and confidence >= 60%
    pub fn is_valid(&self, now_ms: u64) -> bool {
        self.present && self.confidence >= 60 && now_ms.saturating_sub(self.last_update_ms) < 5000
    }
}

static HEART_RATE: Mutex<HeartRateSensor> = Mutex::new(HeartRateSensor::new());

/// Read the current heart rate.  Returns `None` if the reading is stale or
/// the sensor is absent.
pub fn heart_rate_bpm(now_ms: u64) -> Option<u16> {
    let s = HEART_RATE.lock();
    if s.is_valid(now_ms) {
        Some(s.bpm)
    } else {
        None
    }
}

/// Push a new heart-rate reading from the sensor driver.
pub fn heart_rate_update(bpm: u16, confidence: u8, timestamp_ms: u64) {
    let mut s = HEART_RATE.lock();
    s.update(bpm, confidence, timestamp_ms);
}

/// Mark the heart-rate sensor as present (called by hardware init).
pub fn heart_rate_sensor_present(present: bool) {
    HEART_RATE.lock().present = present;
}

// ============================================================================
// Step counter
// ============================================================================

/// Step counter state (pedometer)
pub struct StepCounter {
    /// Total step count since last reset
    pub steps: u32,
    /// Steps in the current 10-minute window (used for cadence)
    pub recent_steps: u16,
    /// Window start timestamp (ms)
    window_start_ms: u64,
    pub present: bool,
}

impl StepCounter {
    const fn new() -> Self {
        StepCounter {
            steps: 0,
            recent_steps: 0,
            window_start_ms: 0,
            present: false,
        }
    }

    /// Add detected steps.  Rolls the window every 10 minutes.
    pub fn add_steps(&mut self, count: u16, now_ms: u64) {
        self.steps = self.steps.saturating_add(count as u32);

        // Roll 10-minute window
        if now_ms.saturating_sub(self.window_start_ms) >= 600_000 {
            self.recent_steps = 0;
            self.window_start_ms = now_ms;
        }
        self.recent_steps = self.recent_steps.saturating_add(count);
    }

    /// Reset step count (e.g., at midnight)
    pub fn reset(&mut self) {
        self.steps = 0;
        self.recent_steps = 0;
    }

    /// Approximate steps per minute (cadence) based on current window
    pub fn cadence_spm(&self, now_ms: u64) -> u16 {
        let elapsed = now_ms.saturating_sub(self.window_start_ms).max(1);
        let spm = (self.recent_steps as u64 * 60_000) / elapsed;
        spm.min(u16::MAX as u64) as u16
    }
}

static STEP_COUNTER: Mutex<StepCounter> = Mutex::new(StepCounter::new());

/// Get total step count since last reset.
pub fn step_count() -> u32 {
    STEP_COUNTER.lock().steps
}

/// Add new steps (called from accelerometer interrupt handler).
pub fn step_counter_add(count: u16, now_ms: u64) {
    STEP_COUNTER.lock().add_steps(count, now_ms);
}

/// Reset the step counter to zero.
pub fn step_counter_reset() {
    STEP_COUNTER.lock().reset();
}

/// Get current walking cadence in steps per minute.
pub fn step_cadence(now_ms: u64) -> u16 {
    STEP_COUNTER.lock().cadence_spm(now_ms)
}

/// Mark step counter hardware as present.
pub fn step_counter_present(present: bool) {
    STEP_COUNTER.lock().present = present;
}

// ============================================================================
// Gyroscope / Orientation sensor
// ============================================================================

/// Three-axis angular velocity in milli-degrees per second.
/// (1000 = 1 °/s)
#[derive(Clone, Copy, Debug, Default)]
pub struct GyroReading {
    /// Rotation around X axis (pitch) in m°/s
    pub x_milli_dps: i32,
    /// Rotation around Y axis (roll) in m°/s
    pub y_milli_dps: i32,
    /// Rotation around Z axis (yaw) in m°/s
    pub z_milli_dps: i32,
    /// Timestamp (kernel uptime ms)
    pub timestamp_ms: u64,
}

/// Orientation computed by integrating gyro + accelerometer
#[derive(Clone, Copy, Debug, Default)]
pub struct Orientation {
    /// Pitch angle in milli-degrees (-90_000 to +90_000)
    pub pitch_milli_deg: i32,
    /// Roll angle in milli-degrees (-180_000 to +180_000)
    pub roll_milli_deg: i32,
    /// Yaw / compass bearing in milli-degrees (0 to 359_999)
    pub yaw_milli_deg: i32,
}

pub struct GyroSensor {
    pub latest: GyroReading,
    pub orientation: Orientation,
    pub present: bool,
}

impl GyroSensor {
    const fn new() -> Self {
        GyroSensor {
            latest: GyroReading {
                x_milli_dps: 0,
                y_milli_dps: 0,
                z_milli_dps: 0,
                timestamp_ms: 0,
            },
            orientation: Orientation {
                pitch_milli_deg: 0,
                roll_milli_deg: 0,
                yaw_milli_deg: 0,
            },
            present: false,
        }
    }

    /// Integrate a new gyro sample into the orientation estimate.
    ///
    /// Uses simple first-order Euler integration:
    ///   angle += angular_velocity × Δt
    ///
    /// `dt_ms` — time since last sample in milliseconds
    pub fn integrate(&mut self, reading: GyroReading, dt_ms: u64) {
        let dt_secs_milli = dt_ms as i64; // ms, so angle += rate[m°/s] * dt[ms] / 1000

        // Clamp dt to 200 ms to avoid wild jumps after a pause
        let dt_clamped = dt_secs_milli.min(200);

        let dpitch = (reading.x_milli_dps as i64 * dt_clamped / 1000) as i32;
        let droll = (reading.y_milli_dps as i64 * dt_clamped / 1000) as i32;
        let dyaw = (reading.z_milli_dps as i64 * dt_clamped / 1000) as i32;

        self.orientation.pitch_milli_deg =
            (self.orientation.pitch_milli_deg + dpitch).clamp(-90_000, 90_000);
        self.orientation.roll_milli_deg =
            (self.orientation.roll_milli_deg + droll).clamp(-180_000, 180_000);

        // Yaw wraps 0–360°
        let new_yaw = self.orientation.yaw_milli_deg + dyaw;
        self.orientation.yaw_milli_deg = new_yaw.rem_euclid(360_000);

        self.latest = reading;
    }
}

static GYRO: Mutex<GyroSensor> = Mutex::new(GyroSensor::new());

/// Push a new gyroscope sample (called from sensor interrupt).
pub fn gyro_update(reading: GyroReading, dt_ms: u64) {
    GYRO.lock().integrate(reading, dt_ms);
}

/// Get the latest orientation estimate.
pub fn gyro_orientation() -> Orientation {
    GYRO.lock().orientation
}

/// Get the raw latest gyro reading.
pub fn gyro_latest() -> GyroReading {
    GYRO.lock().latest
}

/// Mark gyro hardware as present.
pub fn gyro_present(present: bool) {
    GYRO.lock().present = present;
}

// ============================================================================
// GPS sensor
// ============================================================================

/// GPS fix quality
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GpsFix {
    /// No fix
    None,
    /// 2D fix (latitude / longitude, no altitude)
    Fix2D,
    /// 3D fix (latitude / longitude / altitude)
    Fix3D,
    /// DGPS-corrected fix
    Dgps,
}

impl Default for GpsFix {
    fn default() -> Self {
        GpsFix::None
    }
}

/// GPS position data
#[derive(Clone, Copy, Debug, Default)]
pub struct GpsPosition {
    /// Latitude in units of 1e-7 degrees (e.g., 453_123_000 = 45.3123000°)
    pub latitude_1e7: i32,
    /// Longitude in units of 1e-7 degrees
    pub longitude_1e7: i32,
    /// Altitude in millimetres above WGS-84 ellipsoid
    pub altitude_mm: i32,
    /// Ground speed in mm/s
    pub speed_mm_s: u32,
    /// Heading in milli-degrees (0-359,999)
    pub heading_milli_deg: u32,
    /// Horizontal accuracy estimate in mm
    pub hacc_mm: u32,
    /// Fix quality
    pub fix: GpsFix,
    /// Number of satellites used
    pub satellites: u8,
    /// UTC timestamp (Unix epoch seconds, 0 if unknown)
    pub utc_seconds: u64,
}

pub struct GpsSensor {
    pub position: GpsPosition,
    pub last_fix_ms: u64,
    pub present: bool,
}

impl GpsSensor {
    const fn new() -> Self {
        GpsSensor {
            position: GpsPosition {
                latitude_1e7: 0,
                longitude_1e7: 0,
                altitude_mm: 0,
                speed_mm_s: 0,
                heading_milli_deg: 0,
                hacc_mm: 0,
                fix: GpsFix::None,
                satellites: 0,
                utc_seconds: 0,
            },
            last_fix_ms: 0,
            present: false,
        }
    }
}

static GPS: Mutex<GpsSensor> = Mutex::new(GpsSensor::new());

/// Update GPS position (called from NMEA/UBX parser in GPS driver).
pub fn gps_update(pos: GpsPosition, now_ms: u64) {
    let mut g = GPS.lock();
    g.position = pos;
    g.last_fix_ms = now_ms;
}

/// Get the latest GPS position.  Returns `None` if fix is `None` or the
/// reading is stale (> 10 s).
pub fn gps_position(now_ms: u64) -> Option<GpsPosition> {
    let g = GPS.lock();
    if g.position.fix != GpsFix::None && now_ms.saturating_sub(g.last_fix_ms) < 10_000 {
        Some(g.position)
    } else {
        None
    }
}

/// Mark GPS hardware as present.
pub fn gps_present(present: bool) {
    GPS.lock().present = present;
}

// ============================================================================
// Module initialisation
// ============================================================================

/// Initialise all sensor drivers.
///
/// In a real system this would probe the I²C/SPI bus for attached sensors,
/// configure interrupt lines, and start DMA transfers.  Here we set the
/// `present` flags based on a board-capability bitmask.
///
/// `capabilities` bit-flags:
///   bit 0 — heart-rate sensor present
///   bit 1 — step counter present
///   bit 2 — gyroscope present
///   bit 3 — GPS present
pub fn init_with_capabilities(capabilities: u8) {
    heart_rate_sensor_present(capabilities & 0x01 != 0);
    step_counter_present(capabilities & 0x02 != 0);
    gyro_present(capabilities & 0x04 != 0);
    gps_present(capabilities & 0x08 != 0);

    serial_println!(
        "    Wearable/sensors: HR={} steps={} gyro={} GPS={}",
        capabilities & 0x01 != 0,
        capabilities & 0x02 != 0,
        capabilities & 0x04 != 0,
        capabilities & 0x08 != 0,
    );
}

/// Default initialisation — assume all sensors are absent until the hardware
/// driver probes them.
pub fn init() {
    serial_println!("    Wearable/sensors: subsystem initialised (no hardware probed yet)");
}

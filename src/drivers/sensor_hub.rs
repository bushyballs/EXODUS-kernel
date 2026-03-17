use crate::sync::Mutex;
/// Hardware sensor hub driver for Genesis
///
/// Manages inertial measurement unit (IMU) and environmental sensors:
///   - Accelerometer (3-axis, configurable range +/-2/4/8/16g)
///   - Gyroscope (3-axis, configurable range +/-250/500/1000/2000 dps)
///   - Magnetometer (3-axis compass, hard/soft iron calibration)
///   - Sensor fusion (complementary filter for orientation estimation)
///   - Configurable sampling rates (1-8000 Hz)
///   - Per-sensor calibration (offset/scale in Q16 fixed-point)
///   - Threshold-based event reporting (motion, free-fall, tap)
///
/// Communicates via I2C with common IMU chips (MPU6050/BMI160/LSM6DS style).
///
/// Inspired by: Linux IIO subsystem, Android sensor HAL,
/// Bosch BNO055 driver model. All code is original.
use crate::{serial_print, serial_println};
use alloc::collections::VecDeque;

// ---------------------------------------------------------------------------
// Q16 fixed-point
// ---------------------------------------------------------------------------

const Q16_SHIFT: i32 = 16;
const Q16_ONE: i32 = 1 << Q16_SHIFT;

fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> Q16_SHIFT) as i32
}

// ---------------------------------------------------------------------------
// I2C helpers (sensor hub I2C bus)
// ---------------------------------------------------------------------------

const I2C_STATUS_PORT: u16 = 0xC300;
const I2C_DATA_PORT: u16 = 0xC304;
const I2C_CTRL_PORT: u16 = 0xC308;

fn i2c_wait() {
    for _ in 0..5000 {
        if crate::io::inb(I2C_STATUS_PORT) & 0x01 != 0 {
            return;
        }
        core::hint::spin_loop();
    }
}

fn i2c_read(addr: u8, reg: u8) -> u8 {
    crate::io::outb(I2C_CTRL_PORT, 0x01);
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x00);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, reg);
    i2c_wait();
    crate::io::outb(I2C_CTRL_PORT, 0x01);
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x01);
    i2c_wait();
    let val = crate::io::inb(I2C_DATA_PORT);
    crate::io::outb(I2C_CTRL_PORT, 0x02);
    val
}

fn i2c_write(addr: u8, reg: u8, val: u8) {
    crate::io::outb(I2C_CTRL_PORT, 0x01);
    crate::io::outb(I2C_DATA_PORT, (addr << 1) | 0x00);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, reg);
    i2c_wait();
    crate::io::outb(I2C_DATA_PORT, val);
    i2c_wait();
    crate::io::outb(I2C_CTRL_PORT, 0x02);
}

fn i2c_read16(addr: u8, reg: u8) -> i16 {
    let hi = i2c_read(addr, reg) as i16;
    let lo = i2c_read(addr, reg.saturating_add(1)) as i16;
    (hi << 8) | (lo & 0xFF)
}

// ---------------------------------------------------------------------------
// Sensor addresses and chip IDs
// ---------------------------------------------------------------------------

const IMU_ADDR: u8 = 0x68; // MPU6050 / BMI160
const MAG_ADDR: u8 = 0x0C; // AK8963 magnetometer
const IMU_WHO_AM_I: u8 = 0x75;
const MAG_WHO_AM_I: u8 = 0x00;

// MPU6050-style register map
const REG_ACCEL_CFG: u8 = 0x1C;
const REG_GYRO_CFG: u8 = 0x1B;
const REG_SMPLRT_DIV: u8 = 0x19;
const REG_PWR_MGMT1: u8 = 0x6B;
const REG_INT_ENABLE: u8 = 0x38;
const REG_ACCEL_XOUT_H: u8 = 0x3B;
const REG_GYRO_XOUT_H: u8 = 0x43;

// Magnetometer registers (AK8963-style)
const MAG_CTRL1: u8 = 0x0A;
const MAG_DATA_X_LO: u8 = 0x03;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Sensor type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorType {
    Accelerometer,
    Gyroscope,
    Magnetometer,
}

/// Accelerometer full-scale range
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelRange {
    G2 = 0,
    G4 = 1,
    G8 = 2,
    G16 = 3,
}

/// Gyroscope full-scale range
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GyroRange {
    Dps250 = 0,
    Dps500 = 1,
    Dps1000 = 2,
    Dps2000 = 3,
}

/// 3-axis reading in Q16 (units depend on sensor type)
#[derive(Debug, Clone, Copy)]
pub struct AxisReading {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl AxisReading {
    const fn zero() -> Self {
        AxisReading { x: 0, y: 0, z: 0 }
    }
}

/// Calibration data per sensor (offset + scale per axis, all Q16)
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    pub offset: AxisReading,
    pub scale: AxisReading,
}

impl Calibration {
    const fn identity() -> Self {
        Calibration {
            offset: AxisReading::zero(),
            scale: AxisReading {
                x: Q16_ONE,
                y: Q16_ONE,
                z: Q16_ONE,
            },
        }
    }

    fn apply(&self, raw: AxisReading) -> AxisReading {
        AxisReading {
            x: q16_mul(raw.x - self.offset.x, self.scale.x),
            y: q16_mul(raw.y - self.offset.y, self.scale.y),
            z: q16_mul(raw.z - self.offset.z, self.scale.z),
        }
    }
}

/// Orientation from sensor fusion (pitch, roll, yaw in Q16 degrees)
#[derive(Debug, Clone, Copy)]
pub struct Orientation {
    pub pitch_q16: i32,
    pub roll_q16: i32,
    pub yaw_q16: i32,
}

/// Sensor event (threshold crossing, motion, etc.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensorEvent {
    MotionDetected,
    FreeFall,
    SingleTap,
    DoubleTap,
    Stationary,
}

/// Detected IMU chip
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImuChip {
    Mpu6050,
    Bmi160,
    Unknown,
}

// ---------------------------------------------------------------------------
// Inner driver state
// ---------------------------------------------------------------------------

struct SensorHubInner {
    initialized: bool,
    imu_chip: ImuChip,
    has_magnetometer: bool,
    /// Sampling rate divider (output rate = 1kHz / (1 + divider))
    sample_divider: u8,
    /// Accelerometer config
    accel_range: AccelRange,
    accel_cal: Calibration,
    /// Gyroscope config
    gyro_range: GyroRange,
    gyro_cal: Calibration,
    /// Magnetometer calibration
    mag_cal: Calibration,
    /// Latest readings
    accel: AxisReading,
    gyro: AxisReading,
    mag: AxisReading,
    /// Fused orientation (complementary filter)
    orientation: Orientation,
    /// Complementary filter alpha (Q16, 0.98 default)
    filter_alpha: i32,
    /// Event queue
    events: VecDeque<SensorEvent>,
    /// Motion threshold in Q16 (accel magnitude delta)
    motion_threshold: i32,
    /// Previous accel magnitude for motion detection
    prev_accel_mag: i32,
    /// Poll counter
    poll_count: u64,
}

impl SensorHubInner {
    const fn new() -> Self {
        SensorHubInner {
            initialized: false,
            imu_chip: ImuChip::Unknown,
            has_magnetometer: false,
            sample_divider: 9, // 100 Hz default
            accel_range: AccelRange::G2,
            accel_cal: Calibration::identity(),
            gyro_range: GyroRange::Dps250,
            gyro_cal: Calibration::identity(),
            mag_cal: Calibration::identity(),
            accel: AxisReading::zero(),
            gyro: AxisReading::zero(),
            mag: AxisReading::zero(),
            orientation: Orientation {
                pitch_q16: 0,
                roll_q16: 0,
                yaw_q16: 0,
            },
            filter_alpha: 64225, // 0.98 in Q16 = 64225
            events: VecDeque::new(),
            motion_threshold: Q16_ONE / 10, // 0.1g
            prev_accel_mag: Q16_ONE,
            poll_count: 0,
        }
    }

    /// Detect IMU chip
    fn detect(&mut self) {
        let who = i2c_read(IMU_ADDR, IMU_WHO_AM_I);
        match who {
            0x68 => self.imu_chip = ImuChip::Mpu6050,
            0xD1 => self.imu_chip = ImuChip::Bmi160,
            _ => {
                // Try alternate address
                let who2 = i2c_read(IMU_ADDR + 1, IMU_WHO_AM_I);
                if who2 == 0x68 || who2 == 0xD1 {
                    self.imu_chip = if who2 == 0x68 {
                        ImuChip::Mpu6050
                    } else {
                        ImuChip::Bmi160
                    };
                } else {
                    self.imu_chip = ImuChip::Unknown;
                }
            }
        }
        // Check magnetometer
        let mag_id = i2c_read(MAG_ADDR, MAG_WHO_AM_I);
        self.has_magnetometer = mag_id == 0x48; // AK8963
    }

    /// Initialize IMU hardware
    fn init_imu(&self) {
        // Wake from sleep
        i2c_write(IMU_ADDR, REG_PWR_MGMT1, 0x00);
        for _ in 0..10000 {
            core::hint::spin_loop();
        }

        // Set sample rate divider
        i2c_write(IMU_ADDR, REG_SMPLRT_DIV, self.sample_divider);

        // Configure accel range
        i2c_write(IMU_ADDR, REG_ACCEL_CFG, (self.accel_range as u8) << 3);

        // Configure gyro range
        i2c_write(IMU_ADDR, REG_GYRO_CFG, (self.gyro_range as u8) << 3);

        // Enable data ready interrupt
        i2c_write(IMU_ADDR, REG_INT_ENABLE, 0x01);
    }

    /// Initialize magnetometer
    fn init_mag(&self) {
        if !self.has_magnetometer {
            return;
        }
        // Set continuous measurement mode 2 (100 Hz), 16-bit
        i2c_write(MAG_ADDR, MAG_CTRL1, 0x16);
    }

    /// Sensitivity scale factor for accelerometer (Q16, in g per LSB)
    fn accel_sensitivity(&self) -> i32 {
        match self.accel_range {
            AccelRange::G2 => Q16_ONE / 16384, // 16384 LSB/g
            AccelRange::G4 => Q16_ONE / 8192,
            AccelRange::G8 => Q16_ONE / 4096,
            AccelRange::G16 => Q16_ONE / 2048,
        }
    }

    /// Sensitivity scale factor for gyroscope (Q16, in dps per LSB)
    fn gyro_sensitivity(&self) -> i32 {
        match self.gyro_range {
            GyroRange::Dps250 => Q16_ONE / 131,
            GyroRange::Dps500 => Q16_ONE / 66,
            GyroRange::Dps1000 => Q16_ONE / 33,
            GyroRange::Dps2000 => Q16_ONE / 16,
        }
    }

    /// Read accelerometer raw data and convert to Q16 g
    fn read_accel(&mut self) {
        let raw_x = i2c_read16(IMU_ADDR, REG_ACCEL_XOUT_H) as i32;
        let raw_y = i2c_read16(IMU_ADDR, REG_ACCEL_XOUT_H.saturating_add(2)) as i32;
        let raw_z = i2c_read16(IMU_ADDR, REG_ACCEL_XOUT_H.saturating_add(4)) as i32;
        let sens = self.accel_sensitivity();
        let raw = AxisReading {
            x: raw_x * sens,
            y: raw_y * sens,
            z: raw_z * sens,
        };
        self.accel = self.accel_cal.apply(raw);
    }

    /// Read gyroscope raw data and convert to Q16 dps
    fn read_gyro(&mut self) {
        let raw_x = i2c_read16(IMU_ADDR, REG_GYRO_XOUT_H) as i32;
        let raw_y = i2c_read16(IMU_ADDR, REG_GYRO_XOUT_H.saturating_add(2)) as i32;
        let raw_z = i2c_read16(IMU_ADDR, REG_GYRO_XOUT_H.saturating_add(4)) as i32;
        let sens = self.gyro_sensitivity();
        let raw = AxisReading {
            x: raw_x * sens,
            y: raw_y * sens,
            z: raw_z * sens,
        };
        self.gyro = self.gyro_cal.apply(raw);
    }

    /// Read magnetometer
    fn read_mag(&mut self) {
        if !self.has_magnetometer {
            return;
        }
        let raw_x = i2c_read16(MAG_ADDR, MAG_DATA_X_LO) as i32;
        let raw_y = i2c_read16(MAG_ADDR, MAG_DATA_X_LO.saturating_add(2)) as i32;
        let raw_z = i2c_read16(MAG_ADDR, MAG_DATA_X_LO.saturating_add(4)) as i32;
        let raw = AxisReading {
            x: raw_x << (Q16_SHIFT - 8),
            y: raw_y << (Q16_SHIFT - 8),
            z: raw_z << (Q16_SHIFT - 8),
        };
        self.mag = self.mag_cal.apply(raw);
    }

    /// Integer square root
    fn isqrt(val: i32) -> i32 {
        if val <= 0 {
            return 0;
        }
        let mut x = val;
        let mut y = (x + 1) / 2;
        while y < x {
            x = y;
            y = (x + val / x) / 2;
        }
        x
    }

    /// Run complementary filter for orientation fusion
    fn update_orientation(&mut self) {
        // Estimate pitch/roll from accelerometer (atan2 approximation)
        // pitch = atan2(ax, sqrt(ay^2 + az^2))
        // roll  = atan2(ay, sqrt(ax^2 + az^2))
        let ax = self.accel.x;
        let ay = self.accel.y;
        let az = self.accel.z;

        let yz_mag = Self::isqrt(q16_mul(ay, ay).saturating_add(q16_mul(az, az)));
        let xz_mag = Self::isqrt(q16_mul(ax, ax).saturating_add(q16_mul(az, az)));

        // Approximate atan2 as (a/b) * 45 degrees in Q16 for small angles
        let accel_pitch = if yz_mag > 0 {
            (ax * 45 * Q16_ONE) / (yz_mag * 256)
        } else {
            0
        };
        let accel_roll = if xz_mag > 0 {
            (ay * 45 * Q16_ONE) / (xz_mag * 256)
        } else {
            0
        };

        // Complementary filter: orient = alpha * (orient + gyro*dt) + (1-alpha) * accel_orient
        let alpha = self.filter_alpha;
        let one_minus = Q16_ONE.saturating_sub(alpha);

        // Assume dt ~ 10ms (100 Hz), gyro in dps -> degrees = gyro * 0.01
        let gyro_dt = Q16_ONE / 100;
        let gyro_pitch = q16_mul(self.gyro.y, gyro_dt);
        let gyro_roll = q16_mul(self.gyro.x, gyro_dt);
        let gyro_yaw = q16_mul(self.gyro.z, gyro_dt);

        self.orientation.pitch_q16 =
            q16_mul(alpha, self.orientation.pitch_q16.saturating_add(gyro_pitch))
                .saturating_add(q16_mul(one_minus, accel_pitch));
        self.orientation.roll_q16 =
            q16_mul(alpha, self.orientation.roll_q16.saturating_add(gyro_roll))
                .saturating_add(q16_mul(one_minus, accel_roll));
        self.orientation.yaw_q16 = self.orientation.yaw_q16.saturating_add(gyro_yaw);
        // No absolute reference without mag
    }

    /// Detect motion events
    fn detect_events(&mut self) {
        let ax = self.accel.x;
        let ay = self.accel.y;
        let az = self.accel.z;
        let mag = Self::isqrt(
            q16_mul(ax, ax)
                .saturating_add(q16_mul(ay, ay))
                .saturating_add(q16_mul(az, az)),
        );

        let delta = (mag - self.prev_accel_mag).abs();

        // Free-fall: total accel magnitude < 0.3g
        if mag < Q16_ONE / 3 {
            if self.events.len() < 32 {
                self.events.push_back(SensorEvent::FreeFall);
            }
        }
        // Motion detection
        else if delta > self.motion_threshold {
            if self.events.len() < 32 {
                self.events.push_back(SensorEvent::MotionDetected);
            }
        }
        // Tap detection: sharp spike > 2g
        if delta > 2 * Q16_ONE {
            if self.events.len() < 32 {
                self.events.push_back(SensorEvent::SingleTap);
            }
        }

        self.prev_accel_mag = mag;
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HUB: Mutex<SensorHubInner> = Mutex::new(SensorHubInner::new());

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the sensor hub
pub fn init() {
    let mut hub = HUB.lock();
    hub.detect();
    if hub.imu_chip == ImuChip::Unknown {
        serial_println!("  SensorHub: no IMU detected");
        return;
    }
    hub.init_imu();
    hub.init_mag();
    hub.initialized = true;

    let chip_name = match hub.imu_chip {
        ImuChip::Mpu6050 => "MPU6050",
        ImuChip::Bmi160 => "BMI160",
        ImuChip::Unknown => "unknown",
    };
    let mag_str = if hub.has_magnetometer { "+mag" } else { "" };
    let rate = 1000 / (1 + hub.sample_divider as u32);
    serial_println!("  SensorHub: {} {}, {} Hz", chip_name, mag_str, rate);
    drop(hub);
    super::register("sensor-hub", super::DeviceType::Other);
}

/// Poll all sensors and update readings (call from timer or worker)
pub fn poll() {
    let mut hub = HUB.lock();
    if !hub.initialized {
        return;
    }
    hub.read_accel();
    hub.read_gyro();
    hub.read_mag();
    hub.update_orientation();
    hub.detect_events();
    hub.poll_count = hub.poll_count.saturating_add(1);
}

/// Read a sensor's latest 3-axis data (Q16)
pub fn read_sensor(sensor: SensorType) -> AxisReading {
    let hub = HUB.lock();
    match sensor {
        SensorType::Accelerometer => hub.accel,
        SensorType::Gyroscope => hub.gyro,
        SensorType::Magnetometer => hub.mag,
    }
}

/// Get fused orientation (pitch/roll/yaw in Q16 degrees)
pub fn orientation() -> Orientation {
    HUB.lock().orientation
}

/// Set sample rate (1-8000 Hz)
pub fn set_rate(rate_hz: u16) {
    let mut hub = HUB.lock();
    if !hub.initialized {
        return;
    }
    let div = if rate_hz == 0 {
        255
    } else {
        (1000u16 / rate_hz).saturating_sub(1).min(255)
    };
    hub.sample_divider = div as u8;
    i2c_write(IMU_ADDR, REG_SMPLRT_DIV, hub.sample_divider);
}

/// Set accelerometer range
pub fn set_accel_range(range: AccelRange) {
    let mut hub = HUB.lock();
    if !hub.initialized {
        return;
    }
    hub.accel_range = range;
    i2c_write(IMU_ADDR, REG_ACCEL_CFG, (range as u8) << 3);
}

/// Set gyroscope range
pub fn set_gyro_range(range: GyroRange) {
    let mut hub = HUB.lock();
    if !hub.initialized {
        return;
    }
    hub.gyro_range = range;
    i2c_write(IMU_ADDR, REG_GYRO_CFG, (range as u8) << 3);
}

/// Set accelerometer calibration
pub fn set_accel_calibration(cal: Calibration) {
    HUB.lock().accel_cal = cal;
}

/// Set gyroscope calibration
pub fn set_gyro_calibration(cal: Calibration) {
    HUB.lock().gyro_cal = cal;
}

/// Set magnetometer calibration (hard/soft iron correction)
pub fn set_mag_calibration(cal: Calibration) {
    HUB.lock().mag_cal = cal;
}

/// Set motion detection threshold (Q16, in g units)
pub fn set_motion_threshold(threshold_q16: i32) {
    HUB.lock().motion_threshold = threshold_q16;
}

/// Pop the next sensor event
pub fn pop_event() -> Option<SensorEvent> {
    HUB.lock().events.pop_front()
}

/// Get total poll count
pub fn poll_count() -> u64 {
    HUB.lock().poll_count
}

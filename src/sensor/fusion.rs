/// Sensor fusion (combine multiple sensors)
///
/// Part of the AIOS hardware layer.
/// Implements a Madgwick-style complementary filter for fusing accelerometer,
/// gyroscope, and magnetometer data into a quaternion orientation estimate.
/// The filter gain (beta) controls the trade-off between gyro drift correction
/// and accelerometer/magnetometer noise rejection.

use crate::sync::Mutex;

/// Fused orientation (quaternion)
pub struct Orientation {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Fusion algorithm state
pub struct FusionEngine {
    pub orientation: Orientation,
    beta: f32,
    sample_period: f32,
    gyro_bias_x: f32,
    gyro_bias_y: f32,
    gyro_bias_z: f32,
    initialized: bool,
}

static ENGINE: Mutex<Option<FusionEngine>> = Mutex::new(None);

/// Default filter gain (Madgwick beta parameter)
const DEFAULT_BETA: f32 = 0.1;
/// Default sample period in seconds (100 Hz)
const DEFAULT_SAMPLE_PERIOD: f32 = 0.01;

/// Fast inverse square root (Quake III style), sufficient for sensor fusion
fn fast_inv_sqrt(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let half = 0.5 * x;
    let bits = x.to_bits();
    let guess = 0x5f3759df_u32.wrapping_sub(bits >> 1);
    let y = f32::from_bits(guess);
    // One Newton-Raphson iteration
    let y = y * (1.5 - half * y * y);
    // Second iteration for better accuracy
    y * (1.5 - half * y * y)
}

/// Normalize a quaternion in-place
fn normalize_quat(w: &mut f32, x: &mut f32, y: &mut f32, z: &mut f32) {
    let norm = fast_inv_sqrt(*w * *w + *x * *x + *y * *y + *z * *z);
    *w *= norm;
    *x *= norm;
    *y *= norm;
    *z *= norm;
}

/// Update the fusion engine with new sensor readings.
/// accel: [ax, ay, az] in mg (milli-g)
/// gyro: [gx, gy, gz] in mdps (milli-degrees per second)
/// mag: [mx, my, mz] in uT (micro-Tesla)
pub fn update(accel: [f32; 3], gyro: [f32; 3], mag: [f32; 3]) {
    let mut guard = ENGINE.lock();
    let engine = match guard.as_mut() {
        Some(e) => e,
        None => return,
    };

    let q = &mut engine.orientation;
    let dt = engine.sample_period;
    let beta = engine.beta;

    // Convert gyro from mdps to rad/s: mdps * (PI / 180000)
    let gx = (gyro[0] - engine.gyro_bias_x) * 0.00001745329;
    let gy = (gyro[1] - engine.gyro_bias_y) * 0.00001745329;
    let gz = (gyro[2] - engine.gyro_bias_z) * 0.00001745329;

    // Normalize accelerometer
    let a_norm = fast_inv_sqrt(accel[0] * accel[0] + accel[1] * accel[1] + accel[2] * accel[2]);
    if a_norm == 0.0 {
        return;
    }
    let ax = accel[0] * a_norm;
    let ay = accel[1] * a_norm;
    let az = accel[2] * a_norm;

    // Normalize magnetometer (if available)
    let m_sq = mag[0] * mag[0] + mag[1] * mag[1] + mag[2] * mag[2];
    let use_mag = m_sq > 0.01;

    // Gradient descent step (Madgwick simplified, accelerometer only if no mag)
    let qw = q.w;
    let qx = q.x;
    let qy = q.y;
    let qz = q.z;

    // Estimated direction of gravity from quaternion
    let vx = 2.0 * (qx * qz - qw * qy);
    let vy = 2.0 * (qw * qx + qy * qz);
    let vz = qw * qw - qx * qx - qy * qy + qz * qz;

    // Error is cross product of estimated vs measured gravity
    let ex = ay * vz - az * vy;
    let ey = az * vx - ax * vz;
    let ez = ax * vy - ay * vx;

    // If magnetometer data is present, add heading correction
    let (emx, emy, emz) = if use_mag {
        let m_norm = fast_inv_sqrt(m_sq);
        let mx = mag[0] * m_norm;
        let my = mag[1] * m_norm;
        let mz = mag[2] * m_norm;
        // Rotate mag into earth frame
        let hx = mx * (qw * qw + qx * qx - qy * qy - qz * qz)
            + my * 2.0 * (qx * qy - qw * qz)
            + mz * 2.0 * (qx * qz + qw * qy);
        let hy = mx * 2.0 * (qx * qy + qw * qz)
            + my * (qw * qw - qx * qx + qy * qy - qz * qz)
            + mz * 2.0 * (qy * qz - qw * qx);
        // Expected north in horizontal plane
        let bx = (hx * hx + hy * hy).sqrt();
        let bz_est = mz; // simplified
        // Only correct yaw from mag, not pitch/roll
        (0.0, 0.0, (bx * hy - bz_est * hx) * 0.001)
    } else {
        (0.0, 0.0, 0.0)
    };

    // Apply corrections to gyro
    let corrected_gx = gx + beta * (ex + emx);
    let corrected_gy = gy + beta * (ey + emy);
    let corrected_gz = gz + beta * (ez + emz);

    // Quaternion rate of change from corrected gyro
    let qdot_w = 0.5 * (-qx * corrected_gx - qy * corrected_gy - qz * corrected_gz);
    let qdot_x = 0.5 * (qw * corrected_gx + qy * corrected_gz - qz * corrected_gy);
    let qdot_y = 0.5 * (qw * corrected_gy - qx * corrected_gz + qz * corrected_gx);
    let qdot_z = 0.5 * (qw * corrected_gz + qx * corrected_gy - qy * corrected_gx);

    // Integrate
    q.w += qdot_w * dt;
    q.x += qdot_x * dt;
    q.y += qdot_y * dt;
    q.z += qdot_z * dt;

    // Normalize quaternion
    normalize_quat(&mut q.w, &mut q.x, &mut q.y, &mut q.z);
}

pub fn get_orientation() -> Orientation {
    let guard = ENGINE.lock();
    match guard.as_ref() {
        Some(engine) => Orientation {
            w: engine.orientation.w,
            x: engine.orientation.x,
            y: engine.orientation.y,
            z: engine.orientation.z,
        },
        None => Orientation {
            w: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
    }
}

pub fn init() {
    let engine = FusionEngine {
        orientation: Orientation {
            w: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        beta: DEFAULT_BETA,
        sample_period: DEFAULT_SAMPLE_PERIOD,
        gyro_bias_x: 0.0,
        gyro_bias_y: 0.0,
        gyro_bias_z: 0.0,
        initialized: true,
    };
    *ENGINE.lock() = Some(engine);
    crate::serial_println!("  fusion: Madgwick filter initialized (beta={})", DEFAULT_BETA as u32);
}

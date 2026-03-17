use crate::serial_println;
/// Pose estimation stub for Genesis AR
///
/// Estimates the 6-Degrees-of-Freedom (6DoF) camera pose — translation and
/// rotation relative to a world frame — in real-time.  Also provides a
/// simplified body pose skeleton (17-keypoint model) for human pose tracking.
///
/// ## 6DoF Camera Pose
///
/// The camera pose is described by:
///   - Translation vector `(tx, ty, tz)` in millimetres from the world origin.
///   - Rotation represented as a unit quaternion `(qx, qy, qz, qw)`.
///
/// All values are stored as i32 fixed-point × 1_000_000 (so 1.0 = 1_000_000).
///
/// The initial pose is the identity: translation (0,0,0), quaternion (0,0,0,1).
///
/// ## Body Pose Keypoints
///
/// The 17-keypoint model follows the COCO convention:
///   0  Nose
///   1  Left Eye     2  Right Eye
///   3  Left Ear     4  Right Ear
///   5  Left Shoulder  6  Right Shoulder
///   7  Left Elbow     8  Right Elbow
///   9  Left Wrist     10 Right Wrist
///   11 Left Hip       12 Right Hip
///   13 Left Knee      14 Right Knee
///   15 Left Ankle     16 Right Ankle
///
/// Each keypoint has normalised 2D screen coordinates (0–1_000_000) and a
/// visibility flag.
///
/// ## Architecture
///
/// Pose estimation is split across two pipelines:
///
///   1. **Camera pose** — updated by Visual-Inertial Odometry (VIO) combining
///      IMU (gyro + accelerometer) data with camera feature tracking.  In
///      this stub the camera pose is updated via `update_camera_pose()` which
///      is called by the VIO module (or the IMU interrupt handler as a
///      fallback).
///
///   2. **Body pose** — updated by the ML backend after each frame via
///      `submit_body_pose()`.
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;

// ============================================================================
// Fixed-point helpers
// ============================================================================

/// Fixed-point scale: 1.0 = SCALE
const SCALE: i64 = 1_000_000;

/// Fixed-point multiply: (a × b) / SCALE
#[inline]
fn fp_mul(a: i64, b: i64) -> i64 {
    a * b / SCALE
}

// ============================================================================
// Quaternion (unit quaternion for rotation)
// ============================================================================

/// Unit quaternion stored as fixed-point × SCALE.
/// Identity = (0, 0, 0, 1_000_000).
#[derive(Clone, Copy, Debug)]
pub struct Quaternion {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

impl Quaternion {
    pub const IDENTITY: Quaternion = Quaternion {
        x: 0,
        y: 0,
        z: 0,
        w: 1_000_000,
    };

    /// Hamilton product of two quaternions (fixed-point).
    pub fn mul(&self, other: &Quaternion) -> Quaternion {
        let ax = self.x as i64;
        let ay = self.y as i64;
        let az = self.z as i64;
        let aw = self.w as i64;
        let bx = other.x as i64;
        let by = other.y as i64;
        let bz = other.z as i64;
        let bw = other.w as i64;

        Quaternion {
            x: fp_mul(aw, bx) as i32 + fp_mul(ax, bw) as i32 + fp_mul(ay, bz) as i32
                - fp_mul(az, by) as i32,
            y: fp_mul(aw, by) as i32 - fp_mul(ax, bz) as i32
                + fp_mul(ay, bw) as i32
                + fp_mul(az, bx) as i32,
            z: fp_mul(aw, bz) as i32 + fp_mul(ax, by) as i32 - fp_mul(ay, bx) as i32
                + fp_mul(az, bw) as i32,
            w: fp_mul(aw, bw) as i32
                - fp_mul(ax, bx) as i32
                - fp_mul(ay, by) as i32
                - fp_mul(az, bz) as i32,
        }
    }

    /// Conjugate (inverse of unit quaternion)
    pub fn conjugate(&self) -> Quaternion {
        Quaternion {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    /// Squared magnitude × SCALE (should be ≈ SCALE² for a unit quaternion)
    pub fn norm_sq(&self) -> i64 {
        let x = self.x as i64;
        let y = self.y as i64;
        let z = self.z as i64;
        let w = self.w as i64;
        fp_mul(x, x) + fp_mul(y, y) + fp_mul(z, z) + fp_mul(w, w)
    }
}

impl Default for Quaternion {
    fn default() -> Self {
        Quaternion::IDENTITY
    }
}

// ============================================================================
// 3D vector
// ============================================================================

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec3 {
    /// X component in fixed-point mm × 1_000_000
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0, y: 0, z: 0 };

    pub fn add(&self, other: Vec3) -> Vec3 {
        Vec3 {
            x: self.x.saturating_add(other.x),
            y: self.y.saturating_add(other.y),
            z: self.z.saturating_add(other.z),
        }
    }

    /// Length squared in (mm × 1_000_000)²
    pub fn length_sq(&self) -> i64 {
        let x = self.x as i64;
        let y = self.y as i64;
        let z = self.z as i64;
        x * x + y * y + z * z
    }
}

// ============================================================================
// 6DoF Camera pose
// ============================================================================

/// 6-Degrees-of-Freedom camera pose
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraPose {
    /// Translation from world origin in mm (fixed-point × 1_000_000, so
    /// 1.0 = 1 mm; typical values are in the range ±10_000_000 = ±10 m)
    pub translation: Vec3,
    /// Rotation as unit quaternion
    pub rotation: Quaternion,
    /// Timestamp (kernel uptime ms)
    pub timestamp_ms: u64,
    /// Pose confidence: 0 = lost, 1000 = high confidence
    pub confidence: u16,
    /// Tracking quality metric (SLAM feature count, etc.)
    pub tracking_quality: u16,
}

impl CameraPose {
    pub const IDENTITY: CameraPose = CameraPose {
        translation: Vec3::ZERO,
        rotation: Quaternion::IDENTITY,
        timestamp_ms: 0,
        confidence: 0,
        tracking_quality: 0,
    };
}

// ============================================================================
// Body pose keypoints
// ============================================================================

pub const BODY_KEYPOINT_COUNT: usize = 17;

/// Visibility of a body keypoint
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeypointVisibility {
    /// Not detected
    NotVisible,
    /// Visible but low confidence
    LowConfidence,
    /// Visible and reliable
    Visible,
}

impl Default for KeypointVisibility {
    fn default() -> Self {
        KeypointVisibility::NotVisible
    }
}

/// A single body pose keypoint
#[derive(Clone, Copy, Debug, Default)]
pub struct Keypoint {
    /// Normalised x coordinate [0, 1_000_000] left-to-right
    pub x: i32,
    /// Normalised y coordinate [0, 1_000_000] top-to-bottom
    pub y: i32,
    /// Confidence [0, 1000]
    pub confidence: u16,
    pub visibility: KeypointVisibility,
}

/// 17-keypoint body pose skeleton
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyPose {
    pub keypoints: [Keypoint; BODY_KEYPOINT_COUNT],
    /// Bounding box of the full body in normalised coords × 1_000_000
    pub bbox_x: i32,
    pub bbox_y: i32,
    pub bbox_w: i32,
    pub bbox_h: i32,
    /// Timestamp (kernel uptime ms)
    pub timestamp_ms: u64,
    /// Person track ID (stable across frames; 0 = untracked)
    pub track_id: u32,
}

/// Maximum number of simultaneously tracked persons
const MAX_BODY_POSES: usize = 8;

/// COCO keypoint names for debugging
pub const KEYPOINT_NAMES: [&str; BODY_KEYPOINT_COUNT] = [
    "nose",
    "left_eye",
    "right_eye",
    "left_ear",
    "right_ear",
    "left_shoulder",
    "right_shoulder",
    "left_elbow",
    "right_elbow",
    "left_wrist",
    "right_wrist",
    "left_hip",
    "right_hip",
    "left_knee",
    "right_knee",
    "left_ankle",
    "right_ankle",
];

// ============================================================================
// Pose estimation state
// ============================================================================

struct PoseState {
    camera_pose: CameraPose,
    body_poses: [BodyPose; MAX_BODY_POSES],
    body_count: usize,
    /// Camera pose update rate in Hz (informational)
    camera_update_hz: u16,
    /// Body pose update rate in Hz (informational)
    body_update_hz: u16,
    /// Whether camera pose tracking is active
    camera_tracking: bool,
    /// Whether body pose estimation is active
    body_tracking: bool,
    /// Total camera pose updates
    camera_updates: u64,
    /// Total body pose updates
    body_updates: u64,
}

impl PoseState {
    const fn new() -> Self {
        PoseState {
            camera_pose: CameraPose::IDENTITY,
            body_poses: [BodyPose {
                keypoints: [Keypoint {
                    x: 0,
                    y: 0,
                    confidence: 0,
                    visibility: KeypointVisibility::NotVisible,
                }; BODY_KEYPOINT_COUNT],
                bbox_x: 0,
                bbox_y: 0,
                bbox_w: 0,
                bbox_h: 0,
                timestamp_ms: 0,
                track_id: 0,
            }; MAX_BODY_POSES],
            body_count: 0,
            camera_update_hz: 60,
            body_update_hz: 30,
            camera_tracking: false,
            body_tracking: false,
            camera_updates: 0,
            body_updates: 0,
        }
    }
}

static POSE: Mutex<PoseState> = Mutex::new(PoseState::new());

// ============================================================================
// Public API
// ============================================================================

/// Initialise the pose estimation subsystem.
pub fn init() {
    let mut p = POSE.lock();
    p.camera_tracking = true;
    p.body_tracking = true;
    serial_println!("    AR/pose_estimation: 6DoF camera pose + 17-kp body skeleton ready");
}

// --- Camera pose ---

/// Update the 6DoF camera pose (called by VIO / IMU interrupt handler).
pub fn update_camera_pose(pose: CameraPose) {
    let mut p = POSE.lock();
    p.camera_pose = pose;
    p.camera_updates = p.camera_updates.saturating_add(1);
}

/// Get the current camera pose.
pub fn camera_pose() -> CameraPose {
    POSE.lock().camera_pose
}

/// Apply a delta translation to the camera pose (used for IMU-only dead-reckoning).
pub fn apply_translation_delta(dt: Vec3, timestamp_ms: u64) {
    let mut p = POSE.lock();
    p.camera_pose.translation = p.camera_pose.translation.add(dt);
    p.camera_pose.timestamp_ms = timestamp_ms;
    p.camera_updates = p.camera_updates.saturating_add(1);
}

/// Apply a delta rotation to the camera pose (used for gyro integration).
pub fn apply_rotation_delta(dq: Quaternion, timestamp_ms: u64) {
    let mut p = POSE.lock();
    p.camera_pose.rotation = p.camera_pose.rotation.mul(&dq);
    p.camera_pose.timestamp_ms = timestamp_ms;
    p.camera_updates = p.camera_updates.saturating_add(1);
}

/// Reset the camera pose to the world origin.
pub fn reset_camera_pose() {
    let mut p = POSE.lock();
    p.camera_pose = CameraPose::IDENTITY;
}

// --- Body pose ---

/// Submit updated body pose results from the ML backend.
///
/// `poses` — array of detected person poses.
/// `count` — number of valid entries in `poses`.
pub fn submit_body_poses(poses: &[BodyPose], count: usize) {
    let mut p = POSE.lock();
    let n = count.min(MAX_BODY_POSES);
    for i in 0..n {
        p.body_poses[i] = poses[i];
    }
    p.body_count = n;
    p.body_updates = p.body_updates.saturating_add(1);
}

/// Get all currently tracked body poses.
pub fn body_poses() -> ([BodyPose; MAX_BODY_POSES], usize) {
    let p = POSE.lock();
    (p.body_poses, p.body_count)
}

/// Get the body pose for a specific tracking ID.  Returns `None` if not found.
pub fn body_pose_by_track(track_id: u32) -> Option<BodyPose> {
    let p = POSE.lock();
    for i in 0..p.body_count {
        if p.body_poses[i].track_id == track_id {
            return Some(p.body_poses[i]);
        }
    }
    None
}

// --- Utility ---

/// Return a visibility string for a keypoint visibility enum.
pub fn visibility_str(v: KeypointVisibility) -> &'static str {
    match v {
        KeypointVisibility::NotVisible => "none",
        KeypointVisibility::LowConfidence => "low",
        KeypointVisibility::Visible => "good",
    }
}

/// Return the keypoint name by index.
pub fn keypoint_name(idx: usize) -> &'static str {
    KEYPOINT_NAMES.get(idx).copied().unwrap_or("?")
}

/// Get stats: (camera_updates, body_updates)
pub fn stats() -> (u64, u64) {
    let p = POSE.lock();
    (p.camera_updates, p.body_updates)
}

/// Enable / disable camera tracking.
pub fn set_camera_tracking(enabled: bool) {
    POSE.lock().camera_tracking = enabled;
}

/// Enable / disable body pose tracking.
pub fn set_body_tracking(enabled: bool) {
    POSE.lock().body_tracking = enabled;
}

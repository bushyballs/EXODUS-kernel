pub mod ar_renderer;
pub mod camera_overlay;
pub mod gesture_3d;
pub mod hand_tracking;
pub mod object_detection;
pub mod pose_estimation;
pub mod scene_understanding;
/// AR/VR framework for Genesis
///
/// Augmented reality, spatial tracking, plane detection,
/// hand tracking, eye tracking, mixed reality,
/// VR rendering, 6DOF.
///
/// Subsystems:
///   - spatial:           plane detection, spatial anchors, room-scale mapping
///   - hand_tracking:     real-time hand skeleton (21 keypoints per hand)
///   - ar_renderer:       3D rendering, object placement, occlusion, lighting
///   - gesture_3d:        3D gesture recognition from hand tracking
///   - scene_understanding: room mesh, semantics, light estimation
///   - spatial_audio_vr:  6DoF audio spatialiser
///   - camera_overlay:    compositing AR content onto camera feed (new)
///   - object_detection:  real-time object detection with bounding boxes (new)
///   - pose_estimation:   6DoF camera pose + 17-keypoint body pose (new)
///
/// Original implementation for Hoags OS.
pub mod spatial;
pub mod spatial_audio_vr;

use crate::{serial_print, serial_println};

pub fn init() {
    spatial::init();
    hand_tracking::init();
    ar_renderer::init();
    camera_overlay::init();
    object_detection::init();
    pose_estimation::init();
    serial_println!("  AR/VR framework initialized (spatial, hand tracking, renderer, camera overlay, detection, pose)");
}

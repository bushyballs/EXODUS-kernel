use crate::sync::Mutex;
/// Spatial tracking for Genesis AR
///
/// Plane detection, point cloud, anchors,
/// world tracking, image recognition, face mesh.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

#[derive(Clone, Copy, PartialEq)]
pub enum PlaneType {
    Horizontal,
    Vertical,
    Unknown,
}

#[derive(Clone, Copy)]
pub struct Anchor {
    pub id: u32,
    pub x: i32, // mm
    pub y: i32,
    pub z: i32,
    pub rotation_deg: i16,
    pub confidence: u8,
}

struct DetectedPlane {
    id: u32,
    plane_type: PlaneType,
    center_x: i32,
    center_y: i32,
    center_z: i32,
    width_mm: u32,
    height_mm: u32,
    confidence: u8,
}

struct SpatialEngine {
    planes: Vec<DetectedPlane>,
    anchors: Vec<Anchor>,
    next_id: u32,
    tracking_state: TrackingState,
    fps: u16,
}

#[derive(Clone, Copy, PartialEq)]
enum TrackingState {
    NotAvailable,
    Limited,
    Normal,
}

static SPATIAL: Mutex<Option<SpatialEngine>> = Mutex::new(None);

impl SpatialEngine {
    fn new() -> Self {
        SpatialEngine {
            planes: Vec::new(),
            anchors: Vec::new(),
            next_id: 1,
            tracking_state: TrackingState::NotAvailable,
            fps: 60,
        }
    }

    fn add_plane(
        &mut self,
        ptype: PlaneType,
        x: i32,
        y: i32,
        z: i32,
        w: u32,
        h: u32,
        conf: u8,
    ) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.planes.push(DetectedPlane {
            id,
            plane_type: ptype,
            center_x: x,
            center_y: y,
            center_z: z,
            width_mm: w,
            height_mm: h,
            confidence: conf,
        });
        id
    }

    fn create_anchor(&mut self, x: i32, y: i32, z: i32) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.anchors.push(Anchor {
            id,
            x,
            y,
            z,
            rotation_deg: 0,
            confidence: 90,
        });
        id
    }
}

pub fn init() {
    let mut s = SPATIAL.lock();
    *s = Some(SpatialEngine::new());
    serial_println!("    AR: spatial tracking (planes, anchors) ready");
}

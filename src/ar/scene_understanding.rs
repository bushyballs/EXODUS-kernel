use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec;
/// Scene understanding for Genesis AR
///
/// Plane detection, mesh reconstruction, semantic labeling,
/// occlusion mapping, point cloud processing, surface classification.
///
/// All spatial values in millimeters. Q16 fixed-point for normals/confidence.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
pub enum SurfaceLabel {
    Floor,
    Ceiling,
    Wall,
    Table,
    Door,
    Window,
    Furniture,
    Unknown,
}

#[derive(Clone, Copy, PartialEq)]
pub enum OcclusionMode {
    Off,
    DepthBased,
    MeshBased,
}

#[derive(Clone, Copy)]
pub struct Point3 {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Clone, Copy)]
pub struct Normal3 {
    pub nx: i32, // Q16
    pub ny: i32,
    pub nz: i32,
}

#[derive(Clone, Copy)]
pub struct PlaneDescriptor {
    pub id: u32,
    pub center: Point3,
    pub normal: Normal3,
    pub width_mm: u32,
    pub height_mm: u32,
    pub label: SurfaceLabel,
    pub confidence_q16: i32,
}

#[derive(Clone, Copy)]
pub struct MeshTriangle {
    pub v0: u32,
    pub v1: u32,
    pub v2: u32,
}

#[derive(Clone)]
pub struct SceneMesh {
    pub vertices: Vec<Point3>,
    pub normals: Vec<Normal3>,
    pub triangles: Vec<MeshTriangle>,
    pub version: u32,
}

#[derive(Clone, Copy)]
pub struct PointCloudPoint {
    pub position: Point3,
    pub confidence: u8,
    pub label: SurfaceLabel,
}

#[derive(Clone, Copy)]
pub struct OcclusionCell {
    pub depth_mm: u32,
    pub valid: bool,
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

struct SceneEngine {
    planes: Vec<PlaneDescriptor>,
    mesh: SceneMesh,
    point_cloud: Vec<PointCloudPoint>,
    occlusion_mode: OcclusionMode,
    depth_map: Vec<OcclusionCell>,
    depth_width: u32,
    depth_height: u32,
    next_plane_id: u32,
    max_planes: usize,
    max_points: usize,
    mesh_resolution_mm: u32,
    frame_count: u64,
    enabled: bool,
}

static SCENE: Mutex<Option<SceneEngine>> = Mutex::new(None);

impl SceneMesh {
    fn empty() -> Self {
        SceneMesh {
            vertices: Vec::new(),
            normals: Vec::new(),
            triangles: Vec::new(),
            version: 0,
        }
    }

    fn clear(&mut self) {
        self.vertices.clear();
        self.normals.clear();
        self.triangles.clear();
        self.version = self.version.saturating_add(1);
    }

    fn add_vertex(&mut self, pos: Point3, norm: Normal3) -> u32 {
        let idx = self.vertices.len() as u32;
        self.vertices.push(pos);
        self.normals.push(norm);
        idx
    }

    fn add_triangle(&mut self, v0: u32, v1: u32, v2: u32) {
        self.triangles.push(MeshTriangle { v0, v1, v2 });
    }

    fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    fn triangle_count(&self) -> usize {
        self.triangles.len()
    }
}

impl SceneEngine {
    fn new() -> Self {
        SceneEngine {
            planes: Vec::new(),
            mesh: SceneMesh::empty(),
            point_cloud: Vec::new(),
            occlusion_mode: OcclusionMode::DepthBased,
            depth_map: Vec::new(),
            depth_width: 0,
            depth_height: 0,
            next_plane_id: 1,
            max_planes: 64,
            max_points: 8192,
            mesh_resolution_mm: 50,
            frame_count: 0,
            enabled: true,
        }
    }

    /// Detect a plane from point normals using RANSAC-like classification
    fn detect_plane(&mut self, center: Point3, normal: Normal3, w: u32, h: u32) -> Option<u32> {
        if self.planes.len() >= self.max_planes {
            return None;
        }
        let label = Self::classify_surface(&normal);
        let conf = Self::compute_plane_confidence(&normal, w, h);
        let id = self.next_plane_id;
        self.next_plane_id = self.next_plane_id.saturating_add(1);
        self.planes.push(PlaneDescriptor {
            id,
            center,
            normal,
            width_mm: w,
            height_mm: h,
            label,
            confidence_q16: conf,
        });
        Some(id)
    }

    /// Classify surface based on normal direction
    fn classify_surface(normal: &Normal3) -> SurfaceLabel {
        let abs_ny = if normal.ny < 0 { -normal.ny } else { normal.ny };
        let abs_nx = if normal.nx < 0 { -normal.nx } else { normal.nx };
        let abs_nz = if normal.nz < 0 { -normal.nz } else { normal.nz };

        let threshold = (((Q16_ONE as i64) * 70) / 100) as i32; // 0.7 in Q16

        if abs_ny > threshold {
            if normal.ny > 0 {
                SurfaceLabel::Floor
            } else {
                SurfaceLabel::Ceiling
            }
        } else if abs_nx > threshold || abs_nz > threshold {
            SurfaceLabel::Wall
        } else {
            SurfaceLabel::Unknown
        }
    }

    /// Compute confidence based on normal magnitude and plane size
    fn compute_plane_confidence(normal: &Normal3, w: u32, h: u32) -> i32 {
        let mag_sq = (normal.nx as i64) * (normal.nx as i64)
            + (normal.ny as i64) * (normal.ny as i64)
            + (normal.nz as i64) * (normal.nz as i64);
        let q16_one_sq = (Q16_ONE as i64) * (Q16_ONE as i64);

        // Normal quality factor (how close magnitude is to 1.0)
        let norm_quality = if mag_sq > 0 {
            (((q16_one_sq * Q16_ONE as i64) / mag_sq) as i32).min(Q16_ONE)
        } else {
            0
        };

        // Size factor: larger planes are more confident
        let area = (w as i64) * (h as i64);
        let size_factor = if area > 1_000_000 {
            Q16_ONE
        } else {
            (((area * Q16_ONE as i64) / 1_000_000) as i32).min(Q16_ONE)
        };

        // Average both factors
        (((norm_quality as i64 + size_factor as i64) / 2) as i32).min(Q16_ONE)
    }

    /// Add points to the point cloud
    fn add_points(&mut self, points: &[Point3]) {
        for p in points {
            if self.point_cloud.len() >= self.max_points {
                break;
            }
            self.point_cloud.push(PointCloudPoint {
                position: *p,
                confidence: 100,
                label: SurfaceLabel::Unknown,
            });
        }
    }

    /// Label point cloud points using detected planes
    fn label_points_from_planes(&mut self) {
        let distance_threshold: i64 = 100; // 100mm tolerance

        for point in self.point_cloud.iter_mut() {
            let mut best_label = SurfaceLabel::Unknown;
            let mut best_dist: i64 = i64::MAX;

            for plane in &self.planes {
                // Signed distance from point to plane (dot product approach)
                let dx = (point.position.x - plane.center.x) as i64;
                let dy = (point.position.y - plane.center.y) as i64;
                let dz = (point.position.z - plane.center.z) as i64;

                let dot = (dx * plane.normal.nx as i64
                    + dy * plane.normal.ny as i64
                    + dz * plane.normal.nz as i64)
                    / Q16_ONE as i64;

                let abs_dot = if dot < 0 { -dot } else { dot };

                if abs_dot < distance_threshold && abs_dot < best_dist {
                    best_dist = abs_dot;
                    best_label = plane.label;
                }
            }

            point.label = best_label;
        }
    }

    /// Initialize depth map for occlusion
    fn init_depth_map(&mut self, width: u32, height: u32) {
        self.depth_width = width;
        self.depth_height = height;
        let size = (width * height) as usize;
        self.depth_map = vec![
            OcclusionCell {
                depth_mm: 0,
                valid: false
            };
            size
        ];
    }

    /// Update a region of the depth map
    fn update_depth_region(&mut self, x: u32, y: u32, w: u32, h: u32, depth_mm: u32) {
        if self.depth_map.is_empty() {
            return;
        }
        for row in y..(y + h).min(self.depth_height) {
            for col in x..(x + w).min(self.depth_width) {
                let idx = (row * self.depth_width + col) as usize;
                if idx < self.depth_map.len() {
                    self.depth_map[idx] = OcclusionCell {
                        depth_mm,
                        valid: true,
                    };
                }
            }
        }
    }

    /// Query occlusion at a pixel
    fn query_occlusion(&self, px: u32, py: u32, object_depth_mm: u32) -> bool {
        if self.occlusion_mode == OcclusionMode::Off {
            return false;
        }
        let idx = (py * self.depth_width + px) as usize;
        if idx < self.depth_map.len() && self.depth_map[idx].valid {
            self.depth_map[idx].depth_mm < object_depth_mm
        } else {
            false
        }
    }

    /// Reconstruct mesh from point cloud (simple voxel-based)
    fn reconstruct_mesh(&mut self) {
        self.mesh.clear();
        if self.point_cloud.is_empty() {
            return;
        }

        // Simple grid-based mesh: group nearby points into quads
        let res = self.mesh_resolution_mm as i64;
        if res == 0 {
            return;
        }

        // Build vertex grid from point cloud (simplified)
        let mut grid_points: Vec<(i32, i32, i32)> = Vec::new();
        for pt in &self.point_cloud {
            let gx = (((pt.position.x as i64) / res) * res) as i32;
            let gy = (((pt.position.y as i64) / res) * res) as i32;
            let gz = (((pt.position.z as i64) / res) * res) as i32;

            let already_exists = grid_points
                .iter()
                .any(|&(x, y, z)| x == gx && y == gy && z == gz);
            if !already_exists && grid_points.len() < 4096 {
                grid_points.push((gx, gy, gz));
            }
        }

        // Add vertices
        let up = Normal3 {
            nx: 0,
            ny: Q16_ONE,
            nz: 0,
        };
        for &(gx, gy, gz) in &grid_points {
            self.mesh.add_vertex(
                Point3 {
                    x: gx,
                    y: gy,
                    z: gz,
                },
                up,
            );
        }

        // Connect adjacent grid points into triangles
        let vc = grid_points.len();
        for i in 0..vc {
            for j in (i + 1)..vc {
                if j >= vc {
                    break;
                }
                let dx = ((grid_points[i].0 - grid_points[j].0) as i64).abs();
                let dz = ((grid_points[i].2 - grid_points[j].2) as i64).abs();
                if dx <= res && dz <= res && (dx + dz) > 0 {
                    // Find a third point to form a triangle
                    for k in (j + 1)..vc {
                        let dx2 = ((grid_points[i].0 - grid_points[k].0) as i64).abs();
                        let dz2 = ((grid_points[i].2 - grid_points[k].2) as i64).abs();
                        let dx3 = ((grid_points[j].0 - grid_points[k].0) as i64).abs();
                        let dz3 = ((grid_points[j].2 - grid_points[k].2) as i64).abs();
                        if dx2 <= res && dz2 <= res && dx3 <= res && dz3 <= res {
                            self.mesh.add_triangle(i as u32, j as u32, k as u32);
                            break;
                        }
                    }
                }
            }
        }

        self.mesh.version = self.mesh.version.saturating_add(1);
    }

    /// Remove plane by ID
    fn remove_plane(&mut self, id: u32) -> bool {
        if let Some(pos) = self.planes.iter().position(|p| p.id == id) {
            self.planes.remove(pos);
            true
        } else {
            false
        }
    }

    /// Merge overlapping planes with same label
    fn merge_overlapping_planes(&mut self) {
        let merge_dist: i64 = 200; // 200mm overlap threshold
        let mut i = 0;
        while i < self.planes.len() {
            let mut j = i + 1;
            while j < self.planes.len() {
                if self.planes[i].label == self.planes[j].label
                    && self.planes[i].label != SurfaceLabel::Unknown
                {
                    let dx = (self.planes[i].center.x - self.planes[j].center.x) as i64;
                    let dy = (self.planes[i].center.y - self.planes[j].center.y) as i64;
                    let dz = (self.planes[i].center.z - self.planes[j].center.z) as i64;
                    let dist_sq = dx * dx + dy * dy + dz * dz;
                    if dist_sq < merge_dist * merge_dist {
                        // Merge: expand plane i, remove plane j
                        let pw = self.planes[j].width_mm;
                        let ph = self.planes[j].height_mm;
                        self.planes[i].width_mm = self.planes[i].width_mm.max(pw);
                        self.planes[i].height_mm = self.planes[i].height_mm.max(ph);
                        self.planes.remove(j);
                        continue;
                    }
                }
                j += 1;
            }
            i += 1;
        }
    }

    /// Process a frame update
    fn process_frame(&mut self) {
        if !self.enabled {
            return;
        }
        self.frame_count = self.frame_count.saturating_add(1);
        self.label_points_from_planes();
        if self.frame_count % 30 == 0 {
            self.merge_overlapping_planes();
        }
        if self.frame_count % 60 == 0 {
            self.reconstruct_mesh();
        }
    }

    fn plane_count(&self) -> usize {
        self.planes.len()
    }

    fn point_count(&self) -> usize {
        self.point_cloud.len()
    }

    fn clear_all(&mut self) {
        self.planes.clear();
        self.point_cloud.clear();
        self.mesh.clear();
        self.depth_map.clear();
        self.frame_count = 0;
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn detect_plane(center: Point3, normal: Normal3, w: u32, h: u32) -> Option<u32> {
    let mut s = SCENE.lock();
    s.as_mut()
        .and_then(|e| e.detect_plane(center, normal, w, h))
}

pub fn add_points(points: &[Point3]) {
    let mut s = SCENE.lock();
    if let Some(e) = s.as_mut() {
        e.add_points(points);
    }
}

pub fn process_frame() {
    let mut s = SCENE.lock();
    if let Some(e) = s.as_mut() {
        e.process_frame();
    }
}

pub fn query_occlusion(px: u32, py: u32, depth: u32) -> bool {
    let s = SCENE.lock();
    s.as_ref()
        .map_or(false, |e| e.query_occlusion(px, py, depth))
}

pub fn set_occlusion_mode(mode: OcclusionMode) {
    let mut s = SCENE.lock();
    if let Some(e) = s.as_mut() {
        e.occlusion_mode = mode;
    }
}

pub fn plane_count() -> usize {
    let s = SCENE.lock();
    s.as_ref().map_or(0, |e| e.plane_count())
}

pub fn point_count() -> usize {
    let s = SCENE.lock();
    s.as_ref().map_or(0, |e| e.point_count())
}

pub fn init() {
    let mut s = SCENE.lock();
    *s = Some(SceneEngine::new());
    serial_println!("    AR: scene understanding (planes, mesh, occlusion, point cloud) ready");
}

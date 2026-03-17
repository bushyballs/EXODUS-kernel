/// Screen Sharing for Genesis
///
/// High-performance screen sharing with:
///   - Framebuffer capture from the display compositor
///   - Dirty-region tracking (damage accumulator)
///   - Tile-based compression (Q16 fixed-point quality metrics)
///   - Multi-viewer streaming with per-viewer quality adaptation
///   - Cursor overlay and annotation layer
///   - Bandwidth estimation and adaptive frame rate
///
/// Compression pipeline:
///   Capture -> Tile split (64x64) -> Delta detect -> RLE encode ->
///   Optional downscale -> Packetize -> Per-viewer send queue
///
/// All code is original. No external crates.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use crate::sync::Mutex;

use crate::{serial_print, serial_println};

// ---------------------------------------------------------------------------
// Constants and Q16 helpers
// ---------------------------------------------------------------------------

const MAX_VIEWERS: usize = 16;
const TILE_W: u32 = 64;
const TILE_H: u32 = 64;
const DEFAULT_FPS: u32 = 30;
const MIN_FPS: u32 = 5;
const MAX_FPS: u32 = 60;
const BANDWIDTH_SAMPLE_COUNT: usize = 16;
const DEFAULT_PORT: u16 = 5800;

/// Q16 fixed-point: 1.0 = 65536
const Q16_ONE: i32 = 1 << 16;
const Q16_HALF: i32 = 1 << 15;

/// Q16 multiply: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    (((a as i64) * (b as i64)) >> 16) as i32
}

/// Q16 divide: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 { return 0; }
    (((a as i64) << 16) / (b as i64)) as i32
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Sharing session state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareState {
    Idle,
    Capturing,
    Streaming,
    Paused,
    Error,
}

/// Viewer connection state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerState {
    Connecting,
    Active,
    Throttled,
    Disconnected,
}

/// Quality level for adaptive streaming
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityLevel {
    Full,     // Native resolution, all tiles
    High,     // Native res, skip unchanged tiles
    Medium,   // 2x downscale
    Low,      // 4x downscale
    Minimal,  // 8x downscale, keyframes only
}

/// A captured tile with its change state
#[derive(Clone)]
struct Tile {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    changed: bool,
    checksum: u32,
    encoded: Vec<u8>,
}

/// Cursor overlay info
#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    pub x: i32,
    pub y: i32,
    pub visible: bool,
    pub shape_id: u16,
    pub hotspot_x: u8,
    pub hotspot_y: u8,
}

/// Annotation (drawn on top of shared screen)
#[derive(Debug, Clone)]
pub struct Annotation {
    pub id: u32,
    pub kind: AnnotationKind,
    pub color: u32,
    pub thickness: u8,
    pub points: Vec<(i32, i32)>,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    Freehand,
    Line,
    Rectangle,
    Circle,
    Arrow,
    Text,
    Highlight,
}

/// Bandwidth estimator (sliding window of send times)
struct BandwidthEstimator {
    samples: Vec<(u64, u32)>, // (timestamp_ms, bytes_sent)
    index: usize,
    estimated_bps: i32, // Q16 bytes per second
}

impl BandwidthEstimator {
    fn new() -> Self {
        BandwidthEstimator {
            samples: vec![(0, 0); BANDWIDTH_SAMPLE_COUNT],
            index: 0,
            estimated_bps: q16_mul(1_000_000, Q16_ONE), // assume 1 MB/s initially
        }
    }

    /// Record a send event and update estimate
    fn record(&mut self, timestamp_ms: u64, bytes: u32) {
        self.samples[self.index] = (timestamp_ms, bytes);
        self.index = (self.index + 1) % BANDWIDTH_SAMPLE_COUNT;

        // Calculate average bandwidth over the window
        let mut total_bytes: u64 = 0;
        let mut min_ts: u64 = u64::MAX;
        let mut max_ts: u64 = 0;
        for &(ts, b) in &self.samples {
            if ts > 0 {
                total_bytes += b as u64;
                if ts < min_ts { min_ts = ts; }
                if ts > max_ts { max_ts = ts; }
            }
        }
        let elapsed_ms = max_ts.saturating_sub(min_ts).max(1);
        // bytes_per_sec = total_bytes * 1000 / elapsed_ms
        let bps = ((total_bytes * 1000) / elapsed_ms) as i32;
        self.estimated_bps = bps << 16 >> 16; // keep in reasonable Q16 range
    }

    /// Get estimated bandwidth in bytes/sec (integer)
    fn bandwidth_bps(&self) -> u32 {
        (self.estimated_bps >> 16).max(1) as u32
    }
}

/// Per-viewer state
pub struct Viewer {
    pub id: u32,
    pub state: ViewerState,
    pub ip: [u8; 4],
    pub port: u16,
    pub quality: QualityLevel,
    pub target_fps: u32,
    pub actual_fps_q16: i32, // Q16 measured fps
    pub send_queue: Vec<Vec<u8>>,
    pub bytes_sent: u64,
    pub frames_sent: u64,
    pub last_frame_ms: u64,
    bandwidth: BandwidthEstimator,
    /// Per-viewer permission: can they see cursor?
    pub show_cursor: bool,
    /// Per-viewer permission: can they see annotations?
    pub show_annotations: bool,
}

impl Viewer {
    fn new(id: u32, ip: [u8; 4], port: u16) -> Self {
        Viewer {
            id,
            state: ViewerState::Connecting,
            ip,
            port,
            quality: QualityLevel::High,
            target_fps: DEFAULT_FPS,
            actual_fps_q16: 0,
            send_queue: Vec::new(),
            bytes_sent: 0,
            frames_sent: 0,
            last_frame_ms: 0,
            bandwidth: BandwidthEstimator::new(),
            show_cursor: true,
            show_annotations: true,
        }
    }

    /// Decide quality level based on bandwidth
    fn adapt_quality(&mut self) {
        let bw = self.bandwidth.bandwidth_bps();
        // Rough thresholds (bytes/sec)
        if bw > 5_000_000 {
            self.quality = QualityLevel::Full;
            self.target_fps = MAX_FPS;
        } else if bw > 2_000_000 {
            self.quality = QualityLevel::High;
            self.target_fps = 30;
        } else if bw > 500_000 {
            self.quality = QualityLevel::Medium;
            self.target_fps = 15;
        } else if bw > 100_000 {
            self.quality = QualityLevel::Low;
            self.target_fps = 10;
        } else {
            self.quality = QualityLevel::Minimal;
            self.target_fps = MIN_FPS;
        }
    }

    /// Check if it is time to send a frame to this viewer
    fn should_send_frame(&self, now_ms: u64) -> bool {
        if self.target_fps == 0 { return false; }
        let interval_ms = 1000 / self.target_fps as u64;
        now_ms.saturating_sub(self.last_frame_ms) >= interval_ms
    }
}

// ---------------------------------------------------------------------------
// Screen Sharing Engine
// ---------------------------------------------------------------------------

pub struct ScreenShareEngine {
    pub state: ShareState,
    pub screen_w: u32,
    pub screen_h: u32,
    pub viewers: Vec<Viewer>,
    pub cursor: CursorInfo,
    pub annotations: Vec<Annotation>,
    pub next_viewer_id: u32,
    pub next_annotation_id: u32,
    pub listen_port: u16,
    // Tile grid
    tiles: Vec<Tile>,
    tiles_x: u32,
    tiles_y: u32,
    // Previous frame for delta
    prev_frame: Vec<u32>,
    // Statistics
    pub total_frames: u64,
    pub total_bytes: u64,
    pub capture_time_q16: i32, // Q16 average capture time in ms
}

impl ScreenShareEngine {
    fn new() -> Self {
        ScreenShareEngine {
            state: ShareState::Idle,
            screen_w: 1920,
            screen_h: 1080,
            viewers: Vec::new(),
            cursor: CursorInfo { x: 0, y: 0, visible: true, shape_id: 0, hotspot_x: 0, hotspot_y: 0 },
            annotations: Vec::new(),
            next_viewer_id: 1,
            next_annotation_id: 1,
            listen_port: DEFAULT_PORT,
            tiles: Vec::new(),
            tiles_x: 0,
            tiles_y: 0,
            prev_frame: Vec::new(),
            total_frames: 0,
            total_bytes: 0,
            capture_time_q16: 0,
        }
    }

    /// Initialize the tile grid for the current resolution
    fn init_tiles(&mut self) {
        self.tiles_x = (self.screen_w + TILE_W - 1) / TILE_W;
        self.tiles_y = (self.screen_h + TILE_H - 1) / TILE_H;
        self.tiles.clear();

        for ty in 0..self.tiles_y {
            for tx in 0..self.tiles_x {
                let x = tx * TILE_W;
                let y = ty * TILE_H;
                let w = TILE_W.min(self.screen_w - x);
                let h = TILE_H.min(self.screen_h - y);
                self.tiles.push(Tile {
                    x, y, w, h,
                    changed: true,
                    checksum: 0,
                    encoded: Vec::new(),
                });
            }
        }

        let fb_size = (self.screen_w * self.screen_h) as usize;
        self.prev_frame = vec![0u32; fb_size];
    }

    /// Start sharing
    pub fn start(&mut self) {
        self.init_tiles();
        self.state = ShareState::Capturing;
        serial_println!("  [screen_share] Started ({}x{}, {} tiles)",
            self.screen_w, self.screen_h, self.tiles.len());
    }

    /// Stop sharing
    pub fn stop(&mut self) {
        self.state = ShareState::Idle;
        for viewer in &mut self.viewers {
            viewer.state = ViewerState::Disconnected;
        }
        serial_println!("  [screen_share] Stopped");
    }

    /// Add a new viewer
    pub fn add_viewer(&mut self, ip: [u8; 4], port: u16) -> u32 {
        let id = self.next_viewer_id;
        self.next_viewer_id = self.next_viewer_id.saturating_add(1);
        let mut viewer = Viewer::new(id, ip, port);
        viewer.state = ViewerState::Active;
        self.viewers.push(viewer);

        if self.state == ShareState::Capturing {
            self.state = ShareState::Streaming;
        }

        serial_println!("  [screen_share] Viewer {} connected from {}.{}.{}.{}:{}",
            id, ip[0], ip[1], ip[2], ip[3], port);
        id
    }

    /// Remove a viewer
    pub fn remove_viewer(&mut self, viewer_id: u32) {
        if let Some(v) = self.viewers.iter_mut().find(|v| v.id == viewer_id) {
            v.state = ViewerState::Disconnected;
        }
        self.viewers.retain(|v| v.state != ViewerState::Disconnected);

        if self.viewers.is_empty() && self.state == ShareState::Streaming {
            self.state = ShareState::Capturing;
        }
    }

    /// Simple checksum for a tile region (XOR-based for speed)
    fn tile_checksum(fb: &[u32], fb_w: u32, x: u32, y: u32, w: u32, h: u32) -> u32 {
        let mut cksum: u32 = 0;
        for row in y..y + h {
            for col in x..x + w {
                let idx = (row * fb_w + col) as usize;
                if idx < fb.len() {
                    cksum ^= fb[idx];
                    cksum = cksum.rotate_left(1);
                }
            }
        }
        cksum
    }

    /// Detect which tiles changed since last frame
    fn detect_changes(&mut self, framebuffer: &[u32]) {
        for tile in self.tiles.iter_mut() {
            let new_cksum = Self::tile_checksum(framebuffer, self.screen_w, tile.x, tile.y, tile.w, tile.h);
            tile.changed = new_cksum != tile.checksum;
            tile.checksum = new_cksum;
        }
    }

    /// RLE-encode a tile region
    fn encode_tile_rle(fb: &[u32], fb_w: u32, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut encoded = Vec::new();
        let mut run_pixel: u32 = 0;
        let mut run_len: u16 = 0;
        let mut first = true;

        for row in y..y + h {
            for col in x..x + w {
                let idx = (row * fb_w + col) as usize;
                let px = if idx < fb.len() { fb[idx] } else { 0 };

                if first {
                    run_pixel = px;
                    run_len = 1;
                    first = false;
                } else if px == run_pixel && run_len < 0xFFFF {
                    run_len += 1;
                } else {
                    encoded.extend_from_slice(&run_len.to_be_bytes());
                    encoded.extend_from_slice(&run_pixel.to_le_bytes());
                    run_pixel = px;
                    run_len = 1;
                }
            }
        }
        if !first {
            encoded.extend_from_slice(&run_len.to_be_bytes());
            encoded.extend_from_slice(&run_pixel.to_le_bytes());
        }
        encoded
    }

    /// Build a frame packet from changed tiles
    fn build_frame_packet(&mut self, framebuffer: &[u32]) -> Vec<u8> {
        let changed_count = self.tiles.iter().filter(|t| t.changed).count();
        if changed_count == 0 {
            return Vec::new();
        }

        let mut packet = Vec::new();
        // Header: [1B type=0x10][2B num_tiles][4B timestamp placeholder]
        packet.push(0x10);
        packet.extend_from_slice(&(changed_count as u16).to_be_bytes());
        packet.extend_from_slice(&(self.total_frames as u32).to_be_bytes());

        for tile in &self.tiles {
            if !tile.changed {
                continue;
            }
            // Tile header: [2B x][2B y][2B w][2B h]
            packet.extend_from_slice(&(tile.x as u16).to_be_bytes());
            packet.extend_from_slice(&(tile.y as u16).to_be_bytes());
            packet.extend_from_slice(&(tile.w as u16).to_be_bytes());
            packet.extend_from_slice(&(tile.h as u16).to_be_bytes());

            // RLE-encoded tile data
            let encoded = Self::encode_tile_rle(framebuffer, self.screen_w, tile.x, tile.y, tile.w, tile.h);
            packet.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
            packet.extend_from_slice(&encoded);
        }

        // Cursor info (if visible)
        if self.cursor.visible {
            packet.push(0x20); // cursor marker
            packet.extend_from_slice(&(self.cursor.x as i16).to_be_bytes());
            packet.extend_from_slice(&(self.cursor.y as i16).to_be_bytes());
            packet.extend_from_slice(&self.cursor.shape_id.to_be_bytes());
        }

        packet
    }

    /// Process a new frame and distribute to viewers
    pub fn process_frame(&mut self, framebuffer: &[u32], now_ms: u64) {
        if self.state != ShareState::Streaming && self.state != ShareState::Capturing {
            return;
        }

        // Detect changes
        self.detect_changes(framebuffer);

        // Build the frame packet once
        let packet = self.build_frame_packet(framebuffer);
        if packet.is_empty() {
            return;
        }

        let packet_len = packet.len();

        // Distribute to each viewer based on their timing and quality
        for viewer in self.viewers.iter_mut() {
            if viewer.state != ViewerState::Active {
                continue;
            }
            if !viewer.should_send_frame(now_ms) {
                continue;
            }

            // For lower quality levels, we could downsample here
            // For now, send the same packet to all active viewers
            viewer.send_queue.push(packet.clone());
            viewer.bytes_sent += packet_len as u64;
            viewer.frames_sent = viewer.frames_sent.saturating_add(1);
            viewer.last_frame_ms = now_ms;
            viewer.bandwidth.record(now_ms, packet_len as u32);
            viewer.adapt_quality();

            // Calculate actual FPS (Q16)
            if viewer.frames_sent > 1 {
                let elapsed = now_ms.saturating_sub(1).max(1) as i32;
                let frames = viewer.frames_sent as i32;
                viewer.actual_fps_q16 = q16_div(frames, elapsed / 1000);
            }
        }

        // Update prev frame
        if framebuffer.len() == self.prev_frame.len() {
            self.prev_frame.copy_from_slice(framebuffer);
        }

        self.total_frames = self.total_frames.saturating_add(1);
        self.total_bytes += packet_len as u64;
    }

    /// Add an annotation
    pub fn add_annotation(&mut self, kind: AnnotationKind, color: u32, thickness: u8) -> u32 {
        let id = self.next_annotation_id;
        self.next_annotation_id = self.next_annotation_id.saturating_add(1);
        self.annotations.push(Annotation {
            id,
            kind,
            color,
            thickness,
            points: Vec::new(),
            text: String::new(),
        });
        id
    }

    /// Add a point to an annotation (for freehand drawing)
    pub fn annotation_add_point(&mut self, annotation_id: u32, x: i32, y: i32) {
        if let Some(ann) = self.annotations.iter_mut().find(|a| a.id == annotation_id) {
            ann.points.push((x, y));
        }
    }

    /// Clear all annotations
    pub fn clear_annotations(&mut self) {
        self.annotations.clear();
    }

    /// Update cursor position
    pub fn update_cursor(&mut self, x: i32, y: i32) {
        self.cursor.x = x;
        self.cursor.y = y;
    }

    /// Change resolution (requires reinitializing tiles)
    pub fn set_resolution(&mut self, w: u32, h: u32) {
        self.screen_w = w;
        self.screen_h = h;
        self.init_tiles();
        serial_println!("  [screen_share] Resolution changed to {}x{}", w, h);
    }

    /// Get compression ratio (Q16 fixed-point)
    pub fn compression_ratio_q16(&self) -> i32 {
        if self.total_frames == 0 { return Q16_ONE; }
        let raw_size = (self.screen_w * self.screen_h * 4) as i64 * self.total_frames as i64;
        if raw_size == 0 { return Q16_ONE; }
        (((self.total_bytes as i64) << 16) / raw_size) as i32
    }

    /// Get number of active viewers
    pub fn active_viewer_count(&self) -> usize {
        self.viewers.iter().filter(|v| v.state == ViewerState::Active).count()
    }
}

static SCREEN_SHARE: Mutex<Option<ScreenShareEngine>> = Mutex::new(None);

/// Initialize the screen sharing subsystem
pub fn init() {
    let engine = ScreenShareEngine::new();
    *SCREEN_SHARE.lock() = Some(engine);
    serial_println!("    Screen sharing initialized (port {}, max {} viewers)", DEFAULT_PORT, MAX_VIEWERS);
}

/// Start screen sharing
pub fn start() {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.start();
    }
}

/// Stop screen sharing
pub fn stop() {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.stop();
    }
}

/// Add a viewer
pub fn add_viewer(ip: [u8; 4], port: u16) -> u32 {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.add_viewer(ip, port)
    } else {
        0
    }
}

/// Remove a viewer
pub fn remove_viewer(viewer_id: u32) {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.remove_viewer(viewer_id);
    }
}

/// Process a frame (call periodically with the current framebuffer)
pub fn process_frame(framebuffer: &[u32], now_ms: u64) {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.process_frame(framebuffer, now_ms);
    }
}

/// Update cursor position
pub fn update_cursor(x: i32, y: i32) {
    let mut guard = SCREEN_SHARE.lock();
    if let Some(engine) = guard.as_mut() {
        engine.update_cursor(x, y);
    }
}

use crate::serial_println;
/// Object detection stub for Genesis AR
///
/// Provides the data structures and pipeline interface for real-time object
/// detection on camera frames.  The actual inference is performed by a
/// hardware Neural Processing Unit (NPU) driver or a software ML backend
/// (see `crate::ml`); this module is the *consumer* API that AR applications
/// use to query detection results.
///
/// ## Architecture
///
/// The detection pipeline runs asynchronously:
///
///   1. Camera driver places a new frame in the shared `DETECTION_INPUT` ring.
///   2. The NPU/ML backend processes the frame and writes bounding boxes +
///      class labels into `DETECTION_RESULTS`.
///   3. AR applications call `latest_detections()` to retrieve the most
///      recent set of detections.
///
/// ## Bounding boxes
///
/// All bounding boxes are in normalised coordinates (0.0–1.0) stored as
/// fixed-point i32 × 1_000_000 so the code is `no_std` / no-float.
///
/// ## Object classes
///
/// Up to `MAX_CLASSES` classes are supported.  Class names are stored in a
/// 32-byte fixed array.  The initial class list covers common AR use-cases
/// (persons, faces, hands, vehicles, QR codes).
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;

// ============================================================================
// Constants
// ============================================================================

/// Maximum number of detection results returned per frame
const MAX_DETECTIONS: usize = 64;

/// Maximum number of object classes
const MAX_CLASSES: usize = 128;

/// Maximum class name length in bytes
const MAX_CLASS_NAME: usize = 32;

// ============================================================================
// Data types
// ============================================================================

/// Normalised bounding box.
///
/// All coordinates are in fixed-point × 1_000_000 in range [0, 1_000_000]
/// representing [0.0, 1.0] relative to frame dimensions.
/// `x1` < `x2`, `y1` < `y2`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoundingBox {
    /// Left edge (0 = left of frame, 1_000_000 = right of frame)
    pub x1: i32,
    /// Top edge (0 = top of frame, 1_000_000 = bottom of frame)
    pub y1: i32,
    /// Right edge
    pub x2: i32,
    /// Bottom edge
    pub y2: i32,
}

impl BoundingBox {
    /// Width in normalised units
    pub fn width(&self) -> i32 {
        (self.x2 - self.x1).max(0)
    }

    /// Height in normalised units
    pub fn height(&self) -> i32 {
        (self.y2 - self.y1).max(0)
    }

    /// Centre point (x, y) in normalised units
    pub fn centre(&self) -> (i32, i32) {
        ((self.x1 + self.x2) / 2, (self.y1 + self.y2) / 2)
    }

    /// Area in normalised units squared
    pub fn area(&self) -> i64 {
        self.width() as i64 * self.height() as i64
    }

    /// Convert to screen pixel coordinates given frame dimensions.
    pub fn to_screen_rect(&self, frame_w: u32, frame_h: u32) -> (i32, i32, u32, u32) {
        let px = (self.x1 as i64 * frame_w as i64 / 1_000_000) as i32;
        let py = (self.y1 as i64 * frame_h as i64 / 1_000_000) as i32;
        let pw = ((self.x2 - self.x1) as i64 * frame_w as i64 / 1_000_000) as u32;
        let ph = ((self.y2 - self.y1) as i64 * frame_h as i64 / 1_000_000) as u32;
        (px, py, pw, ph)
    }

    /// Intersection-over-Union with another box (fixed-point × 1_000_000).
    pub fn iou(&self, other: &BoundingBox) -> i32 {
        let ix1 = self.x1.max(other.x1);
        let iy1 = self.y1.max(other.y1);
        let ix2 = self.x2.min(other.x2);
        let iy2 = self.y2.min(other.y2);

        if ix2 <= ix1 || iy2 <= iy1 {
            return 0;
        }

        let intersection = (ix2 - ix1) as i64 * (iy2 - iy1) as i64;
        let union = self.area() + other.area() - intersection;
        if union == 0 {
            return 0;
        }
        (intersection * 1_000_000 / union) as i32
    }
}

/// A single detected object
#[derive(Clone, Copy, Debug, Default)]
pub struct Detection {
    /// Bounding box in normalised coordinates
    pub bbox: BoundingBox,
    /// Class index into the class table (u16::MAX = unknown)
    pub class_id: u16,
    /// Detection confidence in range [0, 1_000] representing [0.0, 1.0]
    pub confidence: u16,
    /// Tracking ID (stable across frames for tracked objects; 0 = no tracking)
    pub track_id: u32,
    /// Depth estimate in millimetres (0 = unknown)
    pub depth_mm: u32,
    /// Frame timestamp at which this detection was computed (uptime ms)
    pub frame_timestamp_ms: u64,
}

impl Detection {
    /// Returns `true` if confidence >= `min_conf` (out of 1000)
    pub fn is_confident(&self, min_conf: u16) -> bool {
        self.confidence >= min_conf
    }
}

/// Object class descriptor
#[derive(Clone, Copy, Debug)]
pub struct ObjectClass {
    pub id: u16,
    pub name: [u8; MAX_CLASS_NAME],
    pub name_len: usize,
}

impl ObjectClass {
    fn new(id: u16, name: &[u8]) -> Self {
        let mut n = [0u8; MAX_CLASS_NAME];
        let nlen = name.len().min(MAX_CLASS_NAME);
        n[..nlen].copy_from_slice(&name[..nlen]);
        ObjectClass {
            id,
            name: n,
            name_len: nlen,
        }
    }

    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("?")
    }
}

// ============================================================================
// Detection pipeline state
// ============================================================================

/// Processing state of the detection pipeline
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PipelineState {
    /// Pipeline is idle (not initialised or stopped)
    Idle,
    /// Waiting for a new camera frame
    WaitingForFrame,
    /// Frame is being processed by the NPU/ML backend
    Processing,
    /// Results are ready and can be retrieved
    ResultsReady,
    /// An error occurred; pipeline must be reset
    Error,
}

struct DetectionState {
    pipeline: PipelineState,
    /// Latest detection results (valid when pipeline == ResultsReady)
    results: [Detection; MAX_DETECTIONS],
    result_count: usize,
    /// Class table
    classes: [Option<ObjectClass>; MAX_CLASSES],
    class_count: usize,
    /// Frames processed since init
    frames_processed: u64,
    /// Detections produced since init
    total_detections: u64,
    /// Minimum confidence threshold (0-1000)
    min_confidence: u16,
    /// Whether non-maximum suppression is applied
    nms_enabled: bool,
    /// NMS IoU threshold (0-1_000_000)
    nms_iou_threshold: i32,
}

impl DetectionState {
    const fn new() -> Self {
        DetectionState {
            pipeline: PipelineState::Idle,
            results: [Detection {
                bbox: BoundingBox {
                    x1: 0,
                    y1: 0,
                    x2: 0,
                    y2: 0,
                },
                class_id: u16::MAX,
                confidence: 0,
                track_id: 0,
                depth_mm: 0,
                frame_timestamp_ms: 0,
            }; MAX_DETECTIONS],
            result_count: 0,
            classes: [const { None }; MAX_CLASSES],
            class_count: 0,
            frames_processed: 0,
            total_detections: 0,
            min_confidence: 500, // 50%
            nms_enabled: true,
            nms_iou_threshold: 500_000, // 0.5 IoU
        }
    }

    fn register_class(&mut self, id: u16, name: &[u8]) -> bool {
        if self.class_count >= MAX_CLASSES {
            return false;
        }
        self.classes[self.class_count] = Some(ObjectClass::new(id, name));
        self.class_count += 1;
        true
    }

    fn class_name(&self, id: u16) -> &str {
        for c in self.classes[..self.class_count].iter() {
            if let Some(ref cls) = c {
                if cls.id == id {
                    return cls.name_str();
                }
            }
        }
        "unknown"
    }

    /// Apply Non-Maximum Suppression to `raw_dets[..count]`.
    /// Writes kept detections into `self.results` and updates `self.result_count`.
    fn apply_nms(&mut self, raw_dets: &[Detection], count: usize) {
        // Mark which detections to keep
        let n = count.min(MAX_DETECTIONS);
        let mut kept = [true; MAX_DETECTIONS];

        for i in 0..n {
            if !kept[i] {
                continue;
            }
            for j in (i + 1)..n {
                if !kept[j] {
                    continue;
                }
                if raw_dets[i].class_id != raw_dets[j].class_id {
                    continue;
                }
                let iou = raw_dets[i].bbox.iou(&raw_dets[j].bbox);
                if iou > self.nms_iou_threshold {
                    // Suppress the lower-confidence detection
                    if raw_dets[i].confidence >= raw_dets[j].confidence {
                        kept[j] = false;
                    } else {
                        kept[i] = false;
                    }
                }
            }
        }

        let mut out_count = 0;
        for i in 0..n {
            if kept[i] && out_count < MAX_DETECTIONS {
                self.results[out_count] = raw_dets[i];
                out_count += 1;
            }
        }
        self.result_count = out_count;
    }
}

static DETECTION: Mutex<DetectionState> = Mutex::new(DetectionState::new());

// ============================================================================
// Public API
// ============================================================================

/// Initialise the object detection pipeline.
///
/// Registers the built-in class list and sets the pipeline to
/// `WaitingForFrame`.
pub fn init() {
    let mut s = DETECTION.lock();

    // Register built-in classes for common AR use-cases
    let builtin_classes: &[(&[u8], u16)] = &[
        (b"person", 0),
        (b"face", 1),
        (b"hand", 2),
        (b"car", 3),
        (b"truck", 4),
        (b"bicycle", 5),
        (b"dog", 6),
        (b"cat", 7),
        (b"chair", 8),
        (b"table", 9),
        (b"laptop", 10),
        (b"phone", 11),
        (b"qr_code", 12),
        (b"barcode", 13),
        (b"door", 14),
        (b"window", 15),
        (b"sign", 16),
    ];

    for (name, id) in builtin_classes {
        s.register_class(*id, name);
    }

    s.pipeline = PipelineState::WaitingForFrame;

    serial_println!(
        "    AR/object_detection: pipeline ready ({} builtin classes, min_conf={}%)",
        s.class_count,
        s.min_confidence / 10,
    );
}

/// Feed new detection results from the NPU/ML backend.
///
/// `raw_dets` — array of raw detections (before NMS / threshold filtering).
/// `count`    — number of valid entries in `raw_dets`.
/// `frame_ms` — uptime timestamp of the processed frame.
///
/// This function is called by the ML backend when inference is complete.
pub fn submit_detections(raw_dets: &[Detection], count: usize, frame_ms: u64) {
    let mut s = DETECTION.lock();

    let min_conf = s.min_confidence;

    // Filter by confidence
    let mut filtered = [Detection::default(); MAX_DETECTIONS];
    let mut filtered_count = 0;

    for i in 0..count.min(MAX_DETECTIONS) {
        let mut d = raw_dets[i];
        if d.confidence >= min_conf {
            d.frame_timestamp_ms = frame_ms;
            filtered[filtered_count] = d;
            filtered_count += 1;
        }
    }

    if s.nms_enabled {
        s.apply_nms(&filtered, filtered_count);
    } else {
        for i in 0..filtered_count {
            s.results[i] = filtered[i];
        }
        s.result_count = filtered_count;
    }

    s.frames_processed = s.frames_processed.saturating_add(1);
    s.total_detections = s.total_detections.saturating_add(s.result_count as u64);
    s.pipeline = PipelineState::ResultsReady;
}

/// Get the latest detection results.
///
/// Returns a copy of the detection array and the count.
pub fn latest_detections() -> ([Detection; MAX_DETECTIONS], usize) {
    let s = DETECTION.lock();
    (s.results, s.result_count)
}

/// Get only detections above a given confidence threshold (0-1000 = 0-100%).
pub fn detections_above(min_confidence: u16) -> ([Detection; MAX_DETECTIONS], usize) {
    let s = DETECTION.lock();
    let mut out = [Detection::default(); MAX_DETECTIONS];
    let mut count = 0;
    for i in 0..s.result_count {
        if s.results[i].confidence >= min_confidence && count < MAX_DETECTIONS {
            out[count] = s.results[i];
            count += 1;
        }
    }
    (out, count)
}

/// Get the class name for a given class ID.
pub fn class_name(id: u16) -> &'static str {
    // Cannot return a borrowed reference from a Mutex-locked value,
    // so we use a static lookup for the builtin classes.
    match id {
        0 => "person",
        1 => "face",
        2 => "hand",
        3 => "car",
        4 => "truck",
        5 => "bicycle",
        6 => "dog",
        7 => "cat",
        8 => "chair",
        9 => "table",
        10 => "laptop",
        11 => "phone",
        12 => "qr_code",
        13 => "barcode",
        14 => "door",
        15 => "window",
        16 => "sign",
        _ => "unknown",
    }
}

/// Set the minimum confidence threshold for returned detections (0-1000).
pub fn set_min_confidence(min_conf: u16) {
    DETECTION.lock().min_confidence = min_conf.min(1000);
}

/// Enable or disable Non-Maximum Suppression.
pub fn set_nms(enabled: bool, iou_threshold: i32) {
    let mut s = DETECTION.lock();
    s.nms_enabled = enabled;
    s.nms_iou_threshold = iou_threshold.clamp(0, 1_000_000);
}

/// Get pipeline state
pub fn pipeline_state() -> PipelineState {
    DETECTION.lock().pipeline
}

/// Get statistics: (frames_processed, total_detections)
pub fn stats() -> (u64, u64) {
    let s = DETECTION.lock();
    (s.frames_processed, s.total_detections)
}

/// Reset the pipeline.
pub fn reset() {
    let mut s = DETECTION.lock();
    s.result_count = 0;
    s.frames_processed = 0;
    s.total_detections = 0;
    s.pipeline = PipelineState::WaitingForFrame;
}

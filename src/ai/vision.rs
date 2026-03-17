/// AI Vision for Genesis
///
/// On-device image understanding: object detection,
/// scene recognition, OCR, image captioning, and
/// visual question answering.
///
/// Inspired by: Apple Vision, Google ML Kit. All code is original.
use crate::sync::Mutex;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Detected object in an image
pub struct DetectedObject {
    pub label: String,
    pub confidence: f32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Scene classification result
pub struct SceneResult {
    pub label: String,
    pub confidence: f32,
}

/// OCR text region
pub struct TextRegion {
    pub text: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
    pub language: String,
}

/// Image caption result
pub struct CaptionResult {
    pub caption: String,
    pub confidence: f32,
}

/// Face detection result
pub struct FaceDetection {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub confidence: f32,
    pub landmarks: Vec<(u32, u32)>, // eyes, nose, mouth points
    pub age_estimate: Option<u8>,
    pub smile_score: f32,
}

/// Image similarity result
pub struct SimilarityResult {
    pub image_id: u32,
    pub score: f32,
}

// ---------------------------------------------------------------------------
// Q16 fixed-point helpers (16 fractional bits)
// ---------------------------------------------------------------------------

const Q16_ONE: i32 = 1 << 16;

/// Multiply two Q16 values
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Integer to Q16
fn q16_from_i32(v: i32) -> i32 {
    v << 16
}

/// Q16 to integer (truncating)
fn q16_to_i32(v: i32) -> i32 {
    v >> 16
}

// ---------------------------------------------------------------------------
// Per-cell feature vector used by detection & classification
// ---------------------------------------------------------------------------

/// Simple feature descriptor for an image grid cell
struct CellFeatures {
    avg_brightness: i32,  // Q16, range [0, 255<<16]
    edge_density: i32,    // Q16, fraction of edge pixels
    brightness_var: i32,  // Q16, variance of brightness
    top_half_bright: i32, // Q16, average brightness of top half
    bot_half_bright: i32, // Q16, average brightness of bottom half
}

/// Compute features for a rectangular region of a grayscale buffer.
/// `pixels` is assumed to be 1-byte-per-pixel grayscale (or we take
/// the first byte of each pixel triplet for RGB).  `stride` is the
/// number of bytes per row in the full image.
fn compute_cell_features(
    pixels: &[u8],
    stride: usize,
    rx: usize,
    ry: usize,
    rw: usize,
    rh: usize,
    bytes_per_pixel: usize,
) -> CellFeatures {
    if rw == 0 || rh == 0 {
        return CellFeatures {
            avg_brightness: 0,
            edge_density: 0,
            brightness_var: 0,
            top_half_bright: 0,
            bot_half_bright: 0,
        };
    }

    let mut sum: i64 = 0;
    let mut sum_sq: i64 = 0;
    let mut edge_count: i64 = 0;
    let mut top_sum: i64 = 0;
    let mut bot_sum: i64 = 0;
    let half_h = rh / 2;
    let total = (rw * rh) as i64;

    for dy in 0..rh {
        let row_off = (ry + dy) * stride;
        for dx in 0..rw {
            let idx = row_off + (rx + dx) * bytes_per_pixel;
            if idx >= pixels.len() {
                continue;
            }
            let val = pixels[idx] as i64;
            sum += val;
            sum_sq += val * val;

            if dy < half_h {
                top_sum += val;
            } else {
                bot_sum += val;
            }

            // Simple gradient edge detection (horizontal + vertical)
            if dx > 0 && dy > 0 {
                let left_idx = row_off + (rx + dx - 1) * bytes_per_pixel;
                let up_idx = (ry + dy - 1) * stride + (rx + dx) * bytes_per_pixel;
                if left_idx < pixels.len() && up_idx < pixels.len() {
                    let gx = (val - pixels[left_idx] as i64).abs();
                    let gy = (val - pixels[up_idx] as i64).abs();
                    if gx + gy > 30 {
                        edge_count += 1;
                    }
                }
            }
        }
    }

    let avg = if total > 0 { sum / total } else { 0 };
    let variance = if total > 0 {
        (sum_sq / total) - (avg * avg)
    } else {
        0
    };

    let top_pixels = (rw * half_h) as i64;
    let bot_pixels = (rw * (rh - half_h)) as i64;

    CellFeatures {
        avg_brightness: q16_from_i32(avg as i32),
        edge_density: if total > 0 {
            ((edge_count << 16) / total) as i32
        } else {
            0
        },
        brightness_var: q16_from_i32(variance as i32),
        top_half_bright: if top_pixels > 0 {
            q16_from_i32((top_sum / top_pixels) as i32)
        } else {
            0
        },
        bot_half_bright: if bot_pixels > 0 {
            q16_from_i32((bot_sum / bot_pixels) as i32)
        } else {
            0
        },
    }
}

/// Compute global features across the full image for scene classification
struct GlobalFeatures {
    overall_brightness: i32,   // Q16
    overall_edge_density: i32, // Q16
    brightness_variance: i32,  // Q16
    top_third_bright: i32,     // Q16
    mid_third_bright: i32,     // Q16
    bot_third_bright: i32,     // Q16
    histogram: [i32; 8],       // counts in 8 brightness bins
}

fn compute_global_features(
    pixels: &[u8],
    width: u32,
    height: u32,
    bytes_per_pixel: usize,
) -> GlobalFeatures {
    let w = width as usize;
    let h = height as usize;
    let stride = w * bytes_per_pixel;
    let third = h / 3;

    let mut sum: i64 = 0;
    let mut sum_sq: i64 = 0;
    let mut edge_count: i64 = 0;
    let mut top_sum: i64 = 0;
    let mut mid_sum: i64 = 0;
    let mut bot_sum: i64 = 0;
    let mut histogram = [0i32; 8];

    // Sample every 4th pixel for performance on large images
    let step = if w * h > 100_000 { 4 } else { 1 };
    let mut sampled: i64 = 0;
    let mut top_count: i64 = 0;
    let mut mid_count: i64 = 0;
    let mut bot_count: i64 = 0;

    let mut y = 0;
    while y < h {
        let row_off = y * stride;
        let mut x = 0;
        while x < w {
            let idx = row_off + x * bytes_per_pixel;
            if idx >= pixels.len() {
                x += step;
                continue;
            }
            let val = pixels[idx] as i64;
            sum += val;
            sum_sq += val * val;
            sampled += 1;

            let bin = (val / 32) as usize;
            if bin < 8 {
                histogram[bin] += 1;
            }

            if y < third {
                top_sum += val;
                top_count += 1;
            } else if y < third * 2 {
                mid_sum += val;
                mid_count += 1;
            } else {
                bot_sum += val;
                bot_count += 1;
            }

            // Edge check
            if x > 0 && y > 0 {
                let left = row_off + (x - 1) * bytes_per_pixel;
                let up = (y - 1) * stride + x * bytes_per_pixel;
                if left < pixels.len() && up < pixels.len() {
                    let gx = (val - pixels[left] as i64).abs();
                    let gy = (val - pixels[up] as i64).abs();
                    if gx + gy > 30 {
                        edge_count += 1;
                    }
                }
            }
            x += step;
        }
        y += step;
    }

    let avg = if sampled > 0 { sum / sampled } else { 0 };
    let var = if sampled > 0 {
        (sum_sq / sampled) - avg * avg
    } else {
        0
    };

    GlobalFeatures {
        overall_brightness: q16_from_i32(avg as i32),
        overall_edge_density: if sampled > 0 {
            ((edge_count << 16) / sampled) as i32
        } else {
            0
        },
        brightness_variance: q16_from_i32(var as i32),
        top_third_bright: if top_count > 0 {
            q16_from_i32((top_sum / top_count) as i32)
        } else {
            0
        },
        mid_third_bright: if mid_count > 0 {
            q16_from_i32((mid_sum / mid_count) as i32)
        } else {
            0
        },
        bot_third_bright: if bot_count > 0 {
            q16_from_i32((bot_sum / bot_count) as i32)
        } else {
            0
        },
        histogram,
    }
}

// ---------------------------------------------------------------------------
// Connected component analysis helpers (for OCR)
// ---------------------------------------------------------------------------

/// Simple union-find for connected component labeling
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        let mut parent = Vec::with_capacity(n);
        for i in 0..n {
            parent.push(i);
        }
        UnionFind { parent }
    }

    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        // Path compression
        let mut cur = x;
        while cur != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Bounding box of a connected component
struct ComponentBBox {
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
    pixel_count: u32,
}

// ---------------------------------------------------------------------------
// Vision pipeline
// ---------------------------------------------------------------------------

/// Vision pipeline
pub struct VisionPipeline {
    pub object_labels: Vec<String>,
    pub scene_labels: Vec<String>,
    pub max_detections: usize,
    pub confidence_threshold: f32,
    pub ocr_languages: Vec<String>,
}

impl VisionPipeline {
    const fn new() -> Self {
        VisionPipeline {
            object_labels: Vec::new(),
            scene_labels: Vec::new(),
            max_detections: 20,
            confidence_threshold: 0.5,
            ocr_languages: Vec::new(),
        }
    }

    pub fn load_labels(&mut self) {
        let objects = [
            "person",
            "bicycle",
            "car",
            "motorcycle",
            "bus",
            "truck",
            "traffic light",
            "stop sign",
            "bench",
            "bird",
            "cat",
            "dog",
            "horse",
            "sheep",
            "cow",
            "backpack",
            "umbrella",
            "handbag",
            "bottle",
            "cup",
            "fork",
            "knife",
            "spoon",
            "bowl",
            "banana",
            "apple",
            "sandwich",
            "pizza",
            "chair",
            "couch",
            "potted plant",
            "bed",
            "table",
            "tv",
            "laptop",
            "mouse",
            "keyboard",
            "cell phone",
            "microwave",
            "oven",
            "toaster",
            "sink",
            "refrigerator",
            "book",
            "clock",
            "vase",
            "scissors",
        ];
        for l in &objects {
            self.object_labels.push(String::from(*l));
        }

        let scenes = [
            "indoor",
            "outdoor",
            "beach",
            "mountain",
            "city",
            "forest",
            "office",
            "kitchen",
            "bedroom",
            "bathroom",
            "living room",
            "restaurant",
            "street",
            "highway",
            "park",
            "garden",
            "stadium",
            "airport",
            "store",
            "gym",
        ];
        for s in &scenes {
            self.scene_labels.push(String::from(*s));
        }

        self.ocr_languages = alloc::vec![
            String::from("en"),
            String::from("es"),
            String::from("fr"),
            String::from("de"),
            String::from("ja"),
            String::from("zh"),
        ];
    }

    /// Detect objects in an image (from raw pixel buffer).
    ///
    /// Assumes grayscale (1 byte/pixel) or RGB (3 bytes/pixel).  We auto-detect
    /// based on buffer length vs width*height.
    pub fn detect_objects(&self, pixels: &[u8], width: u32, height: u32) -> Vec<DetectedObject> {
        let w = width as usize;
        let h = height as usize;
        if w == 0 || h == 0 || pixels.is_empty() {
            return Vec::new();
        }

        let bpp = guess_bytes_per_pixel(pixels.len(), w, h);
        let stride = w * bpp;
        let grid = 8usize;
        let cell_w = w / grid;
        let cell_h = h / grid;
        if cell_w == 0 || cell_h == 0 {
            return Vec::new();
        }

        // Compute per-cell features
        let mut cells: Vec<CellFeatures> = Vec::with_capacity(grid * grid);
        for gy in 0..grid {
            for gx in 0..grid {
                let rx = gx * cell_w;
                let ry = gy * cell_h;
                cells.push(compute_cell_features(
                    pixels, stride, rx, ry, cell_w, cell_h, bpp,
                ));
            }
        }

        let mut detections: Vec<DetectedObject> = Vec::new();

        // Score each label against each cell using simple heuristic matching.
        // Merge adjacent high-scoring cells for the same label.
        for label in &self.object_labels {
            let mut best_score: i32 = 0; // Q16
            let mut best_gx: usize = 0;
            let mut best_gy: usize = 0;

            for gy in 0..grid {
                for gx in 0..grid {
                    let cell = &cells[gy * grid + gx];
                    let score = score_cell_for_label(label.as_str(), cell);
                    if score > best_score {
                        best_score = score;
                        best_gx = gx;
                        best_gy = gy;
                    }
                }
            }

            let threshold = q16_mul(Q16_ONE, q16_from_i32(40)) >> 8; // ~0.16 in Q16
            if best_score > threshold {
                let conf = q16_to_confidence(best_score);
                if conf >= self.confidence_threshold {
                    detections.push(DetectedObject {
                        label: label.clone(),
                        confidence: conf,
                        x: (best_gx * cell_w) as u32,
                        y: (best_gy * cell_h) as u32,
                        width: cell_w as u32,
                        height: cell_h as u32,
                    });
                }
            }
        }

        // Sort by confidence descending, truncate
        detections.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        detections.truncate(self.max_detections);
        detections
    }

    /// Classify the scene in an image
    pub fn classify_scene(&self, pixels: &[u8], width: u32, height: u32) -> Vec<SceneResult> {
        let w = width as usize;
        let h = height as usize;
        if w == 0 || h == 0 || pixels.is_empty() {
            return Vec::new();
        }

        let bpp = guess_bytes_per_pixel(pixels.len(), w, h);
        let gf = compute_global_features(pixels, width, height, bpp);

        let mut results: Vec<SceneResult> = Vec::new();

        for label in &self.scene_labels {
            let score = score_scene(label.as_str(), &gf);
            let conf = q16_to_confidence(score);
            if conf >= 0.2 {
                results.push(SceneResult {
                    label: label.clone(),
                    confidence: conf,
                });
            }
        }

        results.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        results.truncate(5);
        results
    }

    /// Perform OCR on an image using binary thresholding + connected components
    pub fn recognize_text(&self, pixels: &[u8], width: u32, height: u32) -> Vec<TextRegion> {
        let w = width as usize;
        let h = height as usize;
        if w == 0 || h == 0 || pixels.is_empty() {
            return Vec::new();
        }

        let bpp = guess_bytes_per_pixel(pixels.len(), w, h);

        // Step 1: compute global average brightness for threshold
        let mut sum: i64 = 0;
        let total = (w * h) as i64;
        let stride = w * bpp;
        for y in 0..h {
            for x in 0..w {
                let idx = y * stride + x * bpp;
                if idx < pixels.len() {
                    sum += pixels[idx] as i64;
                }
            }
        }
        let threshold = if total > 0 { (sum / total) as u8 } else { 128 };

        // Step 2: binary image (true = dark/foreground, assumed text)
        let mut binary = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let idx = y * stride + x * bpp;
                let val = if idx < pixels.len() { pixels[idx] } else { 255 };
                binary.push(val < threshold);
            }
        }

        // Step 3: connected component labeling (two-pass with union-find)
        let mut labels: Vec<usize> = alloc::vec![0; w * h];
        let mut uf = UnionFind::new(w * h + 1);
        let mut next_label: usize = 1;

        // First pass
        for y in 0..h {
            for x in 0..w {
                let pos = y * w + x;
                if !binary[pos] {
                    continue;
                }
                let left = if x > 0 { labels[pos - 1] } else { 0 };
                let up = if y > 0 { labels[pos - w] } else { 0 };

                if left == 0 && up == 0 {
                    if next_label < w * h {
                        labels[pos] = next_label;
                        next_label += 1;
                    }
                } else if left != 0 && up == 0 {
                    labels[pos] = left;
                } else if left == 0 && up != 0 {
                    labels[pos] = up;
                } else {
                    // Both neighbors have labels -- union them
                    let min_l = if left < up { left } else { up };
                    labels[pos] = min_l;
                    uf.union(left, up);
                }
            }
        }

        // Second pass: flatten labels and compute bounding boxes
        let max_components = 512;
        let mut bboxes: Vec<Option<ComponentBBox>> = Vec::new();
        for _ in 0..next_label {
            bboxes.push(None);
        }

        for y in 0..h {
            for x in 0..w {
                let pos = y * w + x;
                let lbl = labels[pos];
                if lbl == 0 {
                    continue;
                }
                let root = uf.find(lbl);
                labels[pos] = root;

                if root >= bboxes.len() {
                    continue;
                }

                match &mut bboxes[root] {
                    Some(bb) => {
                        if (x as u32) < bb.min_x {
                            bb.min_x = x as u32;
                        }
                        if (y as u32) < bb.min_y {
                            bb.min_y = y as u32;
                        }
                        if (x as u32) > bb.max_x {
                            bb.max_x = x as u32;
                        }
                        if (y as u32) > bb.max_y {
                            bb.max_y = y as u32;
                        }
                        bb.pixel_count += 1;
                    }
                    None => {
                        bboxes[root] = Some(ComponentBBox {
                            min_x: x as u32,
                            min_y: y as u32,
                            max_x: x as u32,
                            max_y: y as u32,
                            pixel_count: 1,
                        });
                    }
                }
            }
        }

        // Step 4: filter components that look like character-sized blobs
        // and group horizontally adjacent ones into text regions
        let min_char_w = 3u32;
        let max_char_w = (width / 3).max(1);
        let min_char_h = 5u32;
        let max_char_h = (height / 3).max(1);

        let mut char_bboxes: Vec<ComponentBBox> = Vec::new();
        for bb_opt in &bboxes {
            if let Some(bb) = bb_opt {
                let cw = bb.max_x - bb.min_x + 1;
                let ch = bb.max_y - bb.min_y + 1;
                if cw >= min_char_w
                    && cw <= max_char_w
                    && ch >= min_char_h
                    && ch <= max_char_h
                    && bb.pixel_count >= 4
                {
                    char_bboxes.push(ComponentBBox {
                        min_x: bb.min_x,
                        min_y: bb.min_y,
                        max_x: bb.max_x,
                        max_y: bb.max_y,
                        pixel_count: bb.pixel_count,
                    });
                }
                if char_bboxes.len() >= max_components {
                    break;
                }
            }
        }

        // Sort by y then x for grouping into lines
        char_bboxes.sort_by(|a, b| a.min_y.cmp(&b.min_y).then(a.min_x.cmp(&b.min_x)));

        // Group characters into text line regions
        let mut regions: Vec<TextRegion> = Vec::new();
        let mut used = alloc::vec![false; char_bboxes.len()];
        let line_tolerance = 8u32;

        for i in 0..char_bboxes.len() {
            if used[i] {
                continue;
            }
            used[i] = true;
            let mut rx1 = char_bboxes[i].min_x;
            let mut ry1 = char_bboxes[i].min_y;
            let mut rx2 = char_bboxes[i].max_x;
            let mut ry2 = char_bboxes[i].max_y;
            let mut char_count = 1u32;

            // Merge horizontally adjacent characters on the same line
            for j in (i + 1)..char_bboxes.len() {
                if used[j] {
                    continue;
                }
                let cy = (char_bboxes[j].min_y + char_bboxes[j].max_y) / 2;
                let line_cy = (ry1 + ry2) / 2;
                let y_diff = if cy > line_cy {
                    cy - line_cy
                } else {
                    line_cy - cy
                };
                if y_diff > line_tolerance {
                    continue;
                }
                let gap = if char_bboxes[j].min_x > rx2 {
                    char_bboxes[j].min_x - rx2
                } else {
                    0
                };
                let max_gap = (rx2 - rx1).max(20);
                if gap <= max_gap {
                    used[j] = true;
                    if char_bboxes[j].min_x < rx1 {
                        rx1 = char_bboxes[j].min_x;
                    }
                    if char_bboxes[j].min_y < ry1 {
                        ry1 = char_bboxes[j].min_y;
                    }
                    if char_bboxes[j].max_x > rx2 {
                        rx2 = char_bboxes[j].max_x;
                    }
                    if char_bboxes[j].max_y > ry2 {
                        ry2 = char_bboxes[j].max_y;
                    }
                    char_count += 1;
                }
            }

            if char_count >= 2 {
                let conf = if char_count >= 10 {
                    0.7
                } else {
                    0.4 + (char_count as f32) * 0.03
                };
                regions.push(TextRegion {
                    text: format!("[{} chars]", char_count),
                    x: rx1,
                    y: ry1,
                    width: rx2 - rx1 + 1,
                    height: ry2 - ry1 + 1,
                    confidence: conf,
                    language: String::from("en"),
                });
            }

            if regions.len() >= 64 {
                break;
            }
        }

        regions
    }

    /// Generate an image caption by composing results from other methods
    pub fn caption_image(&self, pixels: &[u8], width: u32, height: u32) -> CaptionResult {
        let scenes = self.classify_scene(pixels, width, height);
        let objects = self.detect_objects(pixels, width, height);
        let faces = self.detect_faces(pixels, width, height);

        let mut parts: Vec<String> = Vec::new();

        // Scene description
        if let Some(scene) = scenes.first() {
            parts.push(format!("{} scene", scene.label));
        }

        // Object mentions
        let obj_limit = 3;
        let mut mentioned = 0;
        for obj in &objects {
            if mentioned >= obj_limit {
                break;
            }
            parts.push(obj.label.clone());
            mentioned += 1;
        }

        // Face mentions
        if !faces.is_empty() {
            if faces.len() == 1 {
                parts.push(String::from("one person"));
            } else {
                parts.push(format!("{} people", faces.len()));
            }
        }

        if parts.is_empty() {
            return CaptionResult {
                caption: String::from("An image"),
                confidence: 0.1,
            };
        }

        let caption = if parts.len() == 1 {
            format!("A {}", parts[0])
        } else {
            let last = parts.pop().unwrap_or_default();
            format!("A {} with {}", parts.join(", "), last)
        };

        let conf = scenes.first().map_or(0.3, |s| s.confidence * 0.5)
            + if objects.is_empty() { 0.0 } else { 0.2 }
            + if faces.is_empty() { 0.0 } else { 0.1 };

        CaptionResult {
            caption,
            confidence: if conf > 1.0 { 1.0 } else { conf },
        }
    }

    /// Detect faces using Haar-like feature scanning.
    ///
    /// Scans the image with a fixed-size window and checks for the
    /// characteristic light/dark pattern of a face (forehead lighter
    /// than eye region, nose lighter than cheeks).
    pub fn detect_faces(&self, pixels: &[u8], width: u32, height: u32) -> Vec<FaceDetection> {
        let w = width as usize;
        let h = height as usize;
        if w < 24 || h < 24 || pixels.is_empty() {
            return Vec::new();
        }

        let bpp = guess_bytes_per_pixel(pixels.len(), w, h);
        let stride = w * bpp;
        let mut detections: Vec<FaceDetection> = Vec::new();

        // Scan at multiple window sizes
        let window_sizes: [usize; 3] = [24, 48, 96];

        for &win_size in &window_sizes {
            if win_size > w || win_size > h {
                continue;
            }
            let step = win_size / 4;
            let mut wy = 0;
            while wy + win_size <= h {
                let mut wx = 0;
                while wx + win_size <= w {
                    let score = haar_face_score(pixels, stride, wx, wy, win_size, bpp);
                    let conf = q16_to_confidence(score);
                    if conf >= self.confidence_threshold {
                        // Suppress duplicates (non-maximum suppression)
                        let overlaps = detections.iter().any(|d| {
                            let dx = d.x as usize;
                            let dy = d.y as usize;
                            let dw = d.width as usize;
                            let dh = d.height as usize;
                            let overlap_x = wx < dx + dw && wx + win_size > dx;
                            let overlap_y = wy < dy + dh && wy + win_size > dy;
                            overlap_x && overlap_y
                        });

                        if !overlaps {
                            let cx = (wx + win_size / 2) as u32;
                            let cy = (wy + win_size / 2) as u32;
                            let third = (win_size / 3) as u32;

                            detections.push(FaceDetection {
                                x: wx as u32,
                                y: wy as u32,
                                width: win_size as u32,
                                height: win_size as u32,
                                confidence: conf,
                                landmarks: alloc::vec![
                                    (cx - third / 2, cy - third / 4), // left eye
                                    (cx + third / 2, cy - third / 4), // right eye
                                    (cx, cy),                         // nose
                                    (cx - third / 3, cy + third / 2), // left mouth
                                    (cx + third / 3, cy + third / 2), // right mouth
                                ],
                                age_estimate: None,
                                smile_score: 0.0,
                            });
                        }
                    }
                    wx += step;
                }
                wy += step;
            }
        }

        detections.truncate(self.max_detections);
        detections
    }

    /// Answer a question about an image (VQA) by running detection + classification
    pub fn visual_qa(&self, pixels: &[u8], width: u32, height: u32, question: &str) -> String {
        let lower = question.to_lowercase();

        if lower.contains("how many") || lower.contains("count") {
            let objects = self.detect_objects(pixels, width, height);
            let faces = self.detect_faces(pixels, width, height);
            if lower.contains("face") || lower.contains("person") || lower.contains("people") {
                return format!("I detect {} face(s) in the image.", faces.len());
            }
            return format!("I detect {} object(s) in the image.", objects.len());
        }

        if lower.contains("what") || lower.contains("describe") {
            let caption = self.caption_image(pixels, width, height);
            return caption.caption;
        }

        if lower.contains("text") || lower.contains("read") || lower.contains("ocr") {
            let regions = self.recognize_text(pixels, width, height);
            if regions.is_empty() {
                return String::from("No text detected in the image.");
            }
            return format!("Found {} text region(s) in the image.", regions.len());
        }

        if lower.contains("scene") || lower.contains("where") {
            let scenes = self.classify_scene(pixels, width, height);
            if let Some(s) = scenes.first() {
                return format!(
                    "This appears to be a {} scene (confidence {:.0}%).",
                    s.label,
                    s.confidence * 100.0
                );
            }
            return String::from("Unable to classify the scene.");
        }

        // Fallback: return a general caption
        let caption = self.caption_image(pixels, width, height);
        format!("Based on my analysis: {}", caption.caption)
    }
}

// ---------------------------------------------------------------------------
// Scoring helpers
// ---------------------------------------------------------------------------

/// Guess bytes per pixel from buffer length
fn guess_bytes_per_pixel(len: usize, w: usize, h: usize) -> usize {
    let total = w * h;
    if total == 0 {
        return 1;
    }
    if len >= total * 4 {
        4 // RGBA
    } else if len >= total * 3 {
        3 // RGB
    } else {
        1 // Grayscale
    }
}

/// Convert a Q16 score (0..Q16_ONE) into a float confidence (0..1)
fn q16_to_confidence(score: i32) -> f32 {
    let clamped = if score < 0 {
        0
    } else if score > Q16_ONE {
        Q16_ONE
    } else {
        score
    };
    (clamped as f32) / (Q16_ONE as f32)
}

/// Score a cell for a given object label using simple heuristics
/// Returns a Q16 score in [0, Q16_ONE].
fn score_cell_for_label(label: &str, cell: &CellFeatures) -> i32 {
    let bright = q16_to_i32(cell.avg_brightness);
    let edge_q = cell.edge_density; // Q16 fraction
    let var = q16_to_i32(cell.brightness_var);

    match label {
        // Dark, high-contrast objects with moderate edges
        "person" | "cat" | "dog" | "horse" | "cow" | "sheep" | "bird" => {
            let mut s: i32 = 0;
            // Moderate brightness, some variance, some edges
            if bright > 40 && bright < 200 {
                s += Q16_ONE / 4;
            }
            if var > 100 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 8 {
                s += Q16_ONE / 4;
            }
            s
        }

        // Vehicles: moderate brightness, high edges
        "car" | "truck" | "bus" | "motorcycle" | "bicycle" => {
            let mut s: i32 = 0;
            if bright > 60 && bright < 220 {
                s += Q16_ONE / 5;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 3;
            }
            if var > 200 {
                s += Q16_ONE / 5;
            }
            s
        }

        // Furniture: moderate brightness, low-moderate edges
        "chair" | "couch" | "bed" | "table" | "bench" => {
            let mut s: i32 = 0;
            if bright > 50 && bright < 200 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 16 && edge_q < Q16_ONE / 4 {
                s += Q16_ONE / 3;
            }
            s
        }

        // Electronics: moderate-high brightness, strong edges
        "tv" | "laptop" | "keyboard" | "cell phone" | "mouse" => {
            let mut s: i32 = 0;
            if bright > 80 {
                s += Q16_ONE / 5;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 3;
            }
            if var > 300 {
                s += Q16_ONE / 6;
            }
            s
        }

        // Food: moderate brightness, low-moderate edges, some color variance
        "banana" | "apple" | "sandwich" | "pizza" | "bowl" | "cup" | "bottle" => {
            let mut s: i32 = 0;
            if bright > 80 && bright < 220 {
                s += Q16_ONE / 4;
            }
            if edge_q < Q16_ONE / 4 {
                s += Q16_ONE / 5;
            }
            if var > 50 && var < 500 {
                s += Q16_ONE / 5;
            }
            s
        }

        // Kitchen items: shiny, high contrast
        "fork" | "knife" | "spoon" | "microwave" | "oven" | "toaster" | "sink" | "refrigerator" => {
            let mut s: i32 = 0;
            if bright > 100 {
                s += Q16_ONE / 5;
            }
            if edge_q > Q16_ONE / 8 {
                s += Q16_ONE / 4;
            }
            if var > 150 {
                s += Q16_ONE / 5;
            }
            s
        }

        // Small misc objects
        "book" | "clock" | "vase" | "scissors" | "backpack" | "umbrella" | "handbag" => {
            let mut s: i32 = 0;
            if bright > 40 && bright < 220 {
                s += Q16_ONE / 5;
            }
            if edge_q > Q16_ONE / 10 {
                s += Q16_ONE / 4;
            }
            s
        }

        // Signs/lights: bright, high contrast
        "traffic light" | "stop sign" => {
            let mut s: i32 = 0;
            if bright > 150 {
                s += Q16_ONE / 3;
            }
            if var > 500 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 5;
            }
            s
        }

        "potted plant" => {
            let mut s: i32 = 0;
            if bright > 50 && bright < 180 {
                s += Q16_ONE / 4;
            }
            if var > 50 {
                s += Q16_ONE / 5;
            }
            s
        }

        _ => 0,
    }
}

/// Score a scene label against global image features. Returns Q16.
fn score_scene(label: &str, gf: &GlobalFeatures) -> i32 {
    let bright = q16_to_i32(gf.overall_brightness);
    let edge_q = gf.overall_edge_density;
    let var = q16_to_i32(gf.brightness_variance);
    let top = q16_to_i32(gf.top_third_bright);
    let mid = q16_to_i32(gf.mid_third_bright);
    let bot = q16_to_i32(gf.bot_third_bright);

    // High brightness bins (bins 5..7 out of 0..7)
    let high_bins: i32 = gf.histogram[5] + gf.histogram[6] + gf.histogram[7];
    let low_bins: i32 = gf.histogram[0] + gf.histogram[1] + gf.histogram[2];
    let total_bins: i32 = gf.histogram.iter().sum();
    let high_frac = if total_bins > 0 {
        (high_bins << 16) / total_bins
    } else {
        0
    };
    let low_frac = if total_bins > 0 {
        (low_bins << 16) / total_bins
    } else {
        0
    };

    match label {
        "outdoor" => {
            let mut s: i32 = 0;
            if bright > 100 {
                s += Q16_ONE / 4;
            }
            if top > mid {
                s += Q16_ONE / 4;
            } // sky brighter than ground
            if var > 200 {
                s += Q16_ONE / 5;
            }
            s
        }
        "indoor" => {
            let mut s: i32 = 0;
            if bright > 60 && bright < 180 {
                s += Q16_ONE / 4;
            }
            if var < 500 {
                s += Q16_ONE / 5;
            }
            if edge_q > Q16_ONE / 8 {
                s += Q16_ONE / 5;
            }
            s
        }
        "beach" => {
            let mut s: i32 = 0;
            if bright > 150 {
                s += Q16_ONE / 3;
            }
            if top > bot + 20 {
                s += Q16_ONE / 4;
            } // bright sky, bright sand
            if high_frac > Q16_ONE / 3 {
                s += Q16_ONE / 5;
            }
            s
        }
        "mountain" => {
            let mut s: i32 = 0;
            if bright > 80 {
                s += Q16_ONE / 5;
            }
            if top > mid && mid > bot {
                s += Q16_ONE / 3;
            } // sky > ridge > base
            if edge_q > Q16_ONE / 10 {
                s += Q16_ONE / 5;
            }
            s
        }
        "city" | "street" | "highway" => {
            let mut s: i32 = 0;
            if edge_q > Q16_ONE / 5 {
                s += Q16_ONE / 3;
            } // lots of edges
            if var > 300 {
                s += Q16_ONE / 4;
            }
            s
        }
        "forest" | "park" | "garden" => {
            let mut s: i32 = 0;
            if bright > 40 && bright < 160 {
                s += Q16_ONE / 4;
            }
            if edge_q < Q16_ONE / 4 {
                s += Q16_ONE / 5;
            }
            if low_frac < Q16_ONE / 4 && high_frac < Q16_ONE / 3 {
                s += Q16_ONE / 5;
            }
            s
        }
        "office" => {
            let mut s: i32 = 0;
            if bright > 120 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 4;
            }
            if var < 600 {
                s += Q16_ONE / 5;
            }
            s
        }
        "kitchen" | "bathroom" => {
            let mut s: i32 = 0;
            if bright > 100 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 4;
            }
            s
        }
        "bedroom" | "living room" => {
            let mut s: i32 = 0;
            if bright > 60 && bright < 180 {
                s += Q16_ONE / 4;
            }
            if edge_q < Q16_ONE / 4 {
                s += Q16_ONE / 4;
            }
            s
        }
        "restaurant" => {
            let mut s: i32 = 0;
            if bright > 50 && bright < 170 {
                s += Q16_ONE / 4;
            }
            if var > 200 {
                s += Q16_ONE / 5;
            }
            s
        }
        "stadium" | "gym" => {
            let mut s: i32 = 0;
            if bright > 120 {
                s += Q16_ONE / 4;
            }
            if var > 400 {
                s += Q16_ONE / 4;
            }
            s
        }
        "airport" | "store" => {
            let mut s: i32 = 0;
            if bright > 130 {
                s += Q16_ONE / 4;
            }
            if edge_q > Q16_ONE / 6 {
                s += Q16_ONE / 4;
            }
            s
        }
        _ => 0,
    }
}

/// Compute a Haar-like face score for a window at (wx, wy) of size `win`.
/// Checks: forehead lighter than eye band, nose region lighter than cheeks.
/// Returns Q16 score.
fn haar_face_score(
    pixels: &[u8],
    stride: usize,
    wx: usize,
    wy: usize,
    win: usize,
    bpp: usize,
) -> i32 {
    let third_h = win / 3;
    let half_w = win / 2;
    let quarter_w = win / 4;

    // Region averages (all in integer brightness 0..255)
    let forehead = region_avg(pixels, stride, wx + quarter_w, wy, half_w, third_h, bpp);
    let left_eye = region_avg(pixels, stride, wx, wy + third_h, half_w, third_h, bpp);
    let right_eye = region_avg(
        pixels,
        stride,
        wx + half_w,
        wy + third_h,
        half_w,
        third_h,
        bpp,
    );
    let nose = region_avg(
        pixels,
        stride,
        wx + quarter_w,
        wy + third_h,
        half_w,
        third_h,
        bpp,
    );
    let left_cheek = region_avg(
        pixels,
        stride,
        wx,
        wy + 2 * third_h,
        quarter_w,
        third_h,
        bpp,
    );
    let right_cheek = region_avg(
        pixels,
        stride,
        wx + win - quarter_w,
        wy + 2 * third_h,
        quarter_w,
        third_h,
        bpp,
    );
    let mouth = region_avg(
        pixels,
        stride,
        wx + quarter_w,
        wy + 2 * third_h,
        half_w,
        third_h,
        bpp,
    );

    let eye_avg = (left_eye + right_eye) / 2;

    let mut score: i32 = 0;

    // Forehead should be brighter than eyes (eye sockets are darker)
    if forehead > eye_avg + 5 {
        score += Q16_ONE / 4;
    }
    // Eyes should be roughly symmetric
    let eye_diff = (left_eye - right_eye).abs();
    if eye_diff < 20 {
        score += Q16_ONE / 5;
    }
    // Nose brighter than cheeks
    if nose > left_cheek && nose > right_cheek {
        score += Q16_ONE / 5;
    }
    // Mouth region darker than nose
    if nose > mouth {
        score += Q16_ONE / 6;
    }
    // Overall brightness in human skin range (rough)
    if forehead > 60 && forehead < 230 {
        score += Q16_ONE / 8;
    }

    score
}

/// Average brightness of a rectangular region
fn region_avg(
    pixels: &[u8],
    stride: usize,
    rx: usize,
    ry: usize,
    rw: usize,
    rh: usize,
    bpp: usize,
) -> i32 {
    if rw == 0 || rh == 0 {
        return 0;
    }
    let mut sum: i64 = 0;
    let mut count: i64 = 0;
    for dy in 0..rh {
        let row = (ry + dy) * stride;
        for dx in 0..rw {
            let idx = row + (rx + dx) * bpp;
            if idx < pixels.len() {
                sum += pixels[idx] as i64;
                count += 1;
            }
        }
    }
    if count > 0 {
        (sum / count) as i32
    } else {
        0
    }
}

static VISION: Mutex<VisionPipeline> = Mutex::new(VisionPipeline::new());

pub fn init() {
    VISION.lock().load_labels();
    crate::serial_println!("    [vision] AI Vision initialized (detect, OCR, caption, VQA)");
}

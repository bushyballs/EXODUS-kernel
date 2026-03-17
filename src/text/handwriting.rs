use crate::sync::Mutex;
use alloc::vec;
/// Hoags Handwriting Recognition - stroke-based template matching
///
/// Recognizes handwritten characters from touch/stylus input using
/// stroke normalization and template matching. Uses geometric feature
/// extraction with Q16 fixed-point math throughout.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

const Q16_ONE: i32 = 65536;
const MAX_POINTS_PER_STROKE: usize = 256;
const RESAMPLE_COUNT: usize = 64;
const MAX_TEMPLATES: usize = 512;
const MAX_STROKES: usize = 8;

#[derive(Clone, Copy)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone)]
pub struct Stroke {
    pub points: Vec<Point>,
}

#[derive(Clone)]
pub struct Template {
    pub label_hash: u64,
    pub points: Vec<Point>,
    pub stroke_count: u8,
    pub aspect_ratio: i32,
    pub is_user_defined: bool,
}

#[derive(Clone, Copy)]
pub struct RecognitionResult {
    pub label_hash: u64,
    pub score: i32,
    pub distance: i32,
}

struct HandwritingEngine {
    templates: Vec<Template>,
    current_strokes: Vec<Stroke>,
    recognition_threshold: i32,
    total_recognized: u64,
    total_rejected: u64,
    templates_trained: u64,
}

static HW_ENGINE: Mutex<Option<HandwritingEngine>> = Mutex::new(None);

impl Point {
    fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }

    fn distance_to(&self, other: &Point) -> i32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dx_sq = ((dx as i64) * (dx as i64)) >> 16;
        let dy_sq = ((dy as i64) * (dy as i64)) >> 16;
        let sum = (dx_sq + dy_sq) as i32;
        q16_sqrt(sum)
    }
}

fn q16_sqrt(val: i32) -> i32 {
    if val <= 0 {
        return 0;
    }
    let mut v = val;
    let mut shift = 0;
    while v > Q16_ONE * 4 {
        v >>= 2;
        shift += 1;
    }
    let mut x = (v + Q16_ONE) / 2;
    for _ in 0..8 {
        if x == 0 {
            break;
        }
        let div = ((v as i64) * (Q16_ONE as i64) / (x as i64)) as i32;
        x = (x + div) / 2;
    }
    x << shift
}

fn path_length(points: &[Point]) -> i32 {
    let mut total: i64 = 0;
    for i in 1..points.len() {
        total += points[i - 1].distance_to(&points[i]) as i64;
    }
    total.min(i32::MAX as i64) as i32
}

fn resample(points: &[Point], n: usize) -> Vec<Point> {
    if points.len() < 2 || n < 2 {
        return points.to_vec();
    }

    let total_len = path_length(points);
    if total_len == 0 {
        return vec![points[0]; n];
    }

    let interval = total_len / (n as i32 - 1);
    if interval == 0 {
        return vec![points[0]; n];
    }

    let mut result = Vec::with_capacity(n);
    result.push(points[0]);

    let mut accumulated: i32 = 0;
    let mut src_idx = 1;

    while result.len() < n && src_idx < points.len() {
        let d = points[src_idx - 1].distance_to(&points[src_idx]);
        if accumulated + d >= interval {
            let overshoot = interval - accumulated;
            let t = if d > 0 {
                ((overshoot as i64) * (Q16_ONE as i64) / (d as i64)) as i32
            } else {
                0
            };
            let nx = points[src_idx - 1].x
                + (((points[src_idx].x - points[src_idx - 1].x) as i64 * t as i64) >> 16) as i32;
            let ny = points[src_idx - 1].y
                + (((points[src_idx].y - points[src_idx - 1].y) as i64 * t as i64) >> 16) as i32;
            result.push(Point::new(nx, ny));
            accumulated = 0;
        } else {
            accumulated += d;
            src_idx += 1;
        }
    }

    while result.len() < n {
        if let Some(&last) = result.last() {
            result.push(last);
        }
    }
    result.truncate(n);
    result
}

fn centroid(points: &[Point]) -> Point {
    if points.is_empty() {
        return Point::new(0, 0);
    }
    let mut sx: i64 = 0;
    let mut sy: i64 = 0;
    for p in points {
        sx += p.x as i64;
        sy += p.y as i64;
    }
    let n = points.len() as i64;
    Point::new((sx / n) as i32, (sy / n) as i32)
}

fn center_at_origin(points: &mut [Point]) {
    let c = centroid(points);
    for p in points.iter_mut() {
        p.x -= c.x;
        p.y -= c.y;
    }
}

fn scale_to_unit(points: &mut [Point]) {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;

    for p in points.iter() {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }

    let width = max_x - min_x;
    let height = max_y - min_y;
    let size = if width > height { width } else { height };
    if size == 0 {
        return;
    }

    for p in points.iter_mut() {
        p.x = ((p.x as i64) * (Q16_ONE as i64) / (size as i64)) as i32;
        p.y = ((p.y as i64) * (Q16_ONE as i64) / (size as i64)) as i32;
    }
}

fn aspect_ratio(points: &[Point]) -> i32 {
    let mut min_x = i32::MAX;
    let mut max_x = i32::MIN;
    let mut min_y = i32::MAX;
    let mut max_y = i32::MIN;
    for p in points {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.y > max_y {
            max_y = p.y;
        }
    }
    let w = max_x - min_x;
    let h = max_y - min_y;
    if h == 0 {
        return Q16_ONE;
    }
    ((w as i64) * (Q16_ONE as i64) / (h as i64)) as i32
}

fn match_distance(a: &[Point], b: &[Point]) -> i32 {
    if a.len() != b.len() {
        return i32::MAX;
    }
    let mut total: i64 = 0;
    for (pa, pb) in a.iter().zip(b.iter()) {
        total += pa.distance_to(pb) as i64;
    }
    let n = a.len() as i64;
    if n == 0 {
        return i32::MAX;
    }
    (total / n).min(i32::MAX as i64) as i32
}

impl Stroke {
    pub fn new() -> Self {
        Stroke { points: Vec::new() }
    }

    pub fn add_point(&mut self, x: i32, y: i32) {
        if self.points.len() < MAX_POINTS_PER_STROKE {
            self.points.push(Point::new(x, y));
        }
    }
}

impl HandwritingEngine {
    fn new() -> Self {
        let mut engine = HandwritingEngine {
            templates: Vec::new(),
            current_strokes: Vec::new(),
            recognition_threshold: Q16_ONE / 2,
            total_recognized: 0,
            total_rejected: 0,
            templates_trained: 0,
        };
        engine.load_default_templates();
        engine
    }

    fn load_default_templates(&mut self) {
        let digit_templates: [(u64, &[(i32, i32)]); 10] = [
            (
                0x3000_0000_0000_0000,
                &[
                    (0, -32768),
                    (23170, -23170),
                    (32768, 0),
                    (23170, 23170),
                    (0, 32768),
                    (-23170, 23170),
                    (-32768, 0),
                    (-23170, -23170),
                ],
            ),
            (
                0x3100_0000_0000_0000,
                &[(0, -32768), (0, -16384), (0, 0), (0, 16384), (0, 32768)],
            ),
            (
                0x3200_0000_0000_0000,
                &[
                    (-16384, -32768),
                    (0, -32768),
                    (16384, -16384),
                    (0, 0),
                    (-16384, 16384),
                    (-16384, 32768),
                    (16384, 32768),
                ],
            ),
            (
                0x3300_0000_0000_0000,
                &[
                    (-16384, -32768),
                    (16384, -16384),
                    (-8192, 0),
                    (16384, 16384),
                    (-16384, 32768),
                ],
            ),
            (
                0x3400_0000_0000_0000,
                &[
                    (-16384, -32768),
                    (-16384, 0),
                    (16384, 0),
                    (16384, -32768),
                    (16384, 32768),
                ],
            ),
            (
                0x3500_0000_0000_0000,
                &[
                    (16384, -32768),
                    (-16384, -32768),
                    (-16384, 0),
                    (16384, 16384),
                    (-16384, 32768),
                ],
            ),
            (
                0x3600_0000_0000_0000,
                &[
                    (16384, -32768),
                    (-16384, 0),
                    (-16384, 16384),
                    (0, 32768),
                    (16384, 16384),
                    (0, 0),
                ],
            ),
            (
                0x3700_0000_0000_0000,
                &[(-16384, -32768), (16384, -32768), (0, 0), (-8192, 32768)],
            ),
            (
                0x3800_0000_0000_0000,
                &[
                    (0, -32768),
                    (16384, -16384),
                    (0, 0),
                    (-16384, 16384),
                    (0, 32768),
                    (16384, 16384),
                    (0, 0),
                    (-16384, -16384),
                ],
            ),
            (
                0x3900_0000_0000_0000,
                &[
                    (0, 0),
                    (-16384, -16384),
                    (0, -32768),
                    (16384, -16384),
                    (0, 0),
                    (16384, 32768),
                ],
            ),
        ];

        for &(label, key_points) in &digit_templates {
            let points: Vec<Point> = key_points.iter().map(|&(x, y)| Point::new(x, y)).collect();
            let ar = aspect_ratio(&points);
            let resampled = resample(&points, RESAMPLE_COUNT);
            self.templates.push(Template {
                label_hash: label,
                points: resampled,
                stroke_count: 1,
                aspect_ratio: ar,
                is_user_defined: false,
            });
        }
    }

    fn begin_stroke(&mut self) {
        if self.current_strokes.len() < MAX_STROKES {
            self.current_strokes.push(Stroke::new());
        }
    }

    fn add_point(&mut self, x: i32, y: i32) {
        if let Some(stroke) = self.current_strokes.last_mut() {
            stroke.add_point(x, y);
        }
    }

    fn recognize(&mut self) -> Vec<RecognitionResult> {
        if self.current_strokes.is_empty() {
            return Vec::new();
        }

        let mut all_points: Vec<Point> = Vec::new();
        let stroke_count = self.current_strokes.len() as u8;
        for stroke in &self.current_strokes {
            all_points.extend_from_slice(&stroke.points);
        }
        if all_points.is_empty() {
            return Vec::new();
        }

        let mut resampled = resample(&all_points, RESAMPLE_COUNT);
        scale_to_unit(&mut resampled);
        center_at_origin(&mut resampled);

        let mut results: Vec<RecognitionResult> = Vec::new();

        for template in &self.templates {
            let stroke_penalty = if template.stroke_count != stroke_count {
                let diff = if template.stroke_count > stroke_count {
                    template.stroke_count - stroke_count
                } else {
                    stroke_count - template.stroke_count
                };
                (diff as i32) * (Q16_ONE / 4)
            } else {
                0
            };

            let dist = match_distance(&resampled, &template.points);
            if dist == i32::MAX {
                continue;
            }

            let total_dist = dist.saturating_add(stroke_penalty);
            let max_dist = Q16_ONE * 2;
            let score = if total_dist >= max_dist {
                0
            } else {
                Q16_ONE - ((total_dist as i64 * Q16_ONE as i64 / max_dist as i64) as i32)
            };

            if score >= self.recognition_threshold {
                results.push(RecognitionResult {
                    label_hash: template.label_hash,
                    score,
                    distance: total_dist,
                });
            }
        }

        results.sort_by(|a, b| b.score.cmp(&a.score));
        results.truncate(5);

        if !results.is_empty() {
            self.total_recognized = self.total_recognized.saturating_add(1);
        } else {
            self.total_rejected = self.total_rejected.saturating_add(1);
        }
        results
    }

    fn clear_strokes(&mut self) {
        self.current_strokes.clear();
    }

    fn train_template(&mut self, label_hash: u64) -> bool {
        if self.templates.len() >= MAX_TEMPLATES {
            return false;
        }
        if self.current_strokes.is_empty() {
            return false;
        }

        let mut all_points: Vec<Point> = Vec::new();
        let stroke_count = self.current_strokes.len() as u8;
        for stroke in &self.current_strokes {
            all_points.extend_from_slice(&stroke.points);
        }
        if all_points.is_empty() {
            return false;
        }

        let ar = aspect_ratio(&all_points);
        let mut resampled = resample(&all_points, RESAMPLE_COUNT);
        scale_to_unit(&mut resampled);
        center_at_origin(&mut resampled);

        self.templates.push(Template {
            label_hash,
            points: resampled,
            stroke_count,
            aspect_ratio: ar,
            is_user_defined: true,
        });
        self.templates_trained = self.templates_trained.saturating_add(1);
        true
    }

    fn get_template_count(&self) -> usize {
        self.templates.len()
    }
}

pub fn begin_stroke() {
    let mut engine = HW_ENGINE.lock();
    if let Some(ref mut hw) = *engine {
        hw.begin_stroke();
    }
}

pub fn add_point(x: i32, y: i32) {
    let mut engine = HW_ENGINE.lock();
    if let Some(ref mut hw) = *engine {
        hw.add_point(x, y);
    }
}

pub fn recognize() -> Vec<RecognitionResult> {
    let mut engine = HW_ENGINE.lock();
    if let Some(ref mut hw) = *engine {
        hw.recognize()
    } else {
        Vec::new()
    }
}

pub fn clear() {
    let mut engine = HW_ENGINE.lock();
    if let Some(ref mut hw) = *engine {
        hw.clear_strokes();
    }
}

pub fn train(label_hash: u64) -> bool {
    let mut engine = HW_ENGINE.lock();
    if let Some(ref mut hw) = *engine {
        hw.train_template(label_hash)
    } else {
        false
    }
}

pub fn init() {
    let engine = HandwritingEngine::new();
    let template_count = engine.get_template_count();
    let mut hw = HW_ENGINE.lock();
    *hw = Some(engine);
    serial_println!(
        "    Handwriting: {} templates, stroke normalization, template matching ready",
        template_count
    );
}

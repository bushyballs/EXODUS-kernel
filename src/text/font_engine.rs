use crate::sync::Mutex;
use alloc::vec;
/// TrueType/OpenType font engine for Genesis
///
/// Implements font face loading, glyph lookup, Bezier curve rasterization,
/// text shaping, kerning, and measurement — all in integer arithmetic.
///
/// Glyph outlines are stored as contours of on-curve and off-curve control
/// points. Quadratic Bezier curves (TrueType) are rasterized using
/// De Casteljau subdivision with Q16 fixed-point math.
///
/// No floating point. No external crates.
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point: 1.0 = 65536
const Q16_ONE: i32 = 65536;

/// Q16 half: 0.5 = 32768
const Q16_HALF: i32 = 32768;

/// Maximum subdivision depth for Bezier rasterization
const MAX_BEZIER_DEPTH: u32 = 8;

/// Default font size in pixels (16px)
const DEFAULT_SIZE_PX: u32 = 16;

/// Maximum number of loaded fonts
const MAX_FONTS: usize = 64;

/// Maximum number of cached glyphs across all fonts
const MAX_GLYPH_CACHE: usize = 2048;

/// FNV-1a offset basis for hashing
const FNV_OFFSET: u64 = 0xCBF29CE484222325;

/// FNV-1a prime for hashing
const FNV_PRIME: u64 = 0x00000100000001B3;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Font style variants
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontStyle {
    Regular,
    Italic,
    Bold,
    BoldItalic,
    Condensed,
    Light,
}

/// A loaded font face with metadata and glyph table.
#[derive(Debug, Clone)]
pub struct FontFace {
    /// Unique identifier for this font face
    pub id: u32,
    /// Hash of the font family name
    pub family_hash: u64,
    /// Style variant
    pub style: FontStyle,
    /// Weight value (100-900, 400 = normal, 700 = bold)
    pub weight: u16,
    /// Total number of glyphs in the font
    pub glyph_count: u32,
    /// Design units per em square
    pub units_per_em: u16,
    /// Typographic ascender in font units
    pub ascender: i16,
    /// Typographic descender in font units (negative)
    pub descender: i16,
    /// Line gap in font units
    pub line_gap: i16,
    /// Current rendering size in pixels
    pub size_px: u32,
    /// Scale factor: Q16 value = (size_px * Q16_ONE) / units_per_em
    pub scale_q16: i32,
    /// Glyph storage for this face
    pub glyphs: Vec<Glyph>,
    /// Kerning pairs: (left_codepoint, right_codepoint, adjustment)
    pub kern_pairs: Vec<(u32, u32, i16)>,
}

/// A single glyph with outline and metrics.
#[derive(Debug, Clone)]
pub struct Glyph {
    /// Unicode codepoint this glyph represents
    pub codepoint: u32,
    /// Advance width in font units
    pub advance_width: i16,
    /// Left side bearing in font units
    pub lsb: i16,
    /// Contours that make up the outline
    pub contours: Vec<GlyphContour>,
}

/// A single contour (closed path) of a glyph outline.
#[derive(Debug, Clone)]
pub struct GlyphContour {
    /// Control points as (x, y) in font units
    pub points: Vec<(i32, i32)>,
    /// Whether each point is on-curve (true) or an off-curve control point (false)
    pub on_curve: Vec<bool>,
}

/// Rasterized glyph bitmap.
#[derive(Debug, Clone)]
pub struct RasterGlyph {
    /// Codepoint of the source glyph
    pub codepoint: u32,
    /// Bitmap width in pixels
    pub width: u32,
    /// Bitmap height in pixels
    pub height: u32,
    /// Horizontal bearing X (offset from origin to left edge) in pixels
    pub bearing_x: i32,
    /// Horizontal bearing Y (offset from origin to top edge) in pixels
    pub bearing_y: i32,
    /// Advance width in pixels (Q16 for sub-pixel accuracy)
    pub advance_q16: i32,
    /// Grayscale bitmap data (0 = transparent, 255 = fully opaque)
    pub bitmap: Vec<u8>,
}

/// Line metrics for text layout.
#[derive(Debug, Clone, Copy)]
pub struct LineMetrics {
    /// Ascender in pixels (Q16)
    pub ascender_q16: i32,
    /// Descender in pixels (Q16, negative)
    pub descender_q16: i32,
    /// Line gap in pixels (Q16)
    pub line_gap_q16: i32,
    /// Total line height in pixels (Q16)
    pub line_height_q16: i32,
}

/// Text measurement result.
#[derive(Debug, Clone, Copy)]
pub struct TextMetrics {
    /// Total advance width in pixels (Q16)
    pub width_q16: i32,
    /// Height from ascender to descender (Q16)
    pub height_q16: i32,
    /// Number of glyphs shaped
    pub glyph_count: u32,
}

/// Shaped glyph with position information.
#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    /// Codepoint
    pub codepoint: u32,
    /// X offset from text origin in pixels (Q16)
    pub x_offset_q16: i32,
    /// Y offset from text origin in pixels (Q16)
    pub y_offset_q16: i32,
    /// Advance to next glyph in pixels (Q16)
    pub advance_q16: i32,
}

/// Cached rasterized glyph entry.
#[derive(Debug, Clone)]
struct GlyphCacheEntry {
    font_id: u32,
    size_px: u32,
    codepoint: u32,
    raster: RasterGlyph,
    access_count: u64,
}

/// Font engine state holding all loaded fonts and the glyph cache.
pub struct FontEngine {
    /// All loaded font faces
    pub fonts: Vec<FontFace>,
    /// Next font ID to assign
    next_font_id: u32,
    /// Active font index
    active_font: usize,
    /// Rasterized glyph cache
    glyph_cache: Vec<GlyphCacheEntry>,
    /// Monotonic access counter for cache eviction
    access_counter: u64,
}

/// Global font engine instance.
pub static FONT_ENGINE: Mutex<Option<FontEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// FNV-1a hash helper
// ---------------------------------------------------------------------------

/// Compute FNV-1a hash of a byte slice.
fn fnv1a_hash(data: &[u8]) -> u64 {
    let mut hash: u64 = FNV_OFFSET;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ---------------------------------------------------------------------------
// Q16 fixed-point math helpers
// ---------------------------------------------------------------------------

/// Multiply two Q16 values: (a * b) >> 16
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Divide two Q16 values: (a << 16) / b
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / (b as i64)) as i32
}

/// Integer square root using Newton's method (no floating point).
fn isqrt(val: i64) -> i64 {
    if val <= 0 {
        return 0;
    }
    if val == 1 {
        return 1;
    }
    let mut x = val;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + val / x) / 2;
    }
    x
}

/// Linear interpolation between two Q16 values.
/// t is Q16 (0 = a, Q16_ONE = b).
fn q16_lerp(a: i32, b: i32, t: i32) -> i32 {
    a + q16_mul(b - a, t)
}

// ---------------------------------------------------------------------------
// Bezier curve rasterization
// ---------------------------------------------------------------------------

/// Flatten a quadratic Bezier curve into line segments via subdivision.
/// Appends points to `output`. Points are in Q16 font-unit coordinates.
fn flatten_bezier_quad(
    p0x: i32,
    p0y: i32,
    p1x: i32,
    p1y: i32,
    p2x: i32,
    p2y: i32,
    output: &mut Vec<(i32, i32)>,
    depth: u32,
) {
    // Check if the curve is flat enough to approximate with a line
    let mid_x = (p0x + p2x) / 2;
    let mid_y = (p0y + p2y) / 2;
    let dx = p1x - mid_x;
    let dy = p1y - mid_y;
    let dist_sq = (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64);

    // Flatness threshold: 1/4 pixel in Q16 units squared
    let threshold: i64 = 256;

    if dist_sq <= threshold || depth >= MAX_BEZIER_DEPTH {
        output.push((p2x, p2y));
        return;
    }

    // De Casteljau subdivision at t=0.5
    let m01x = (p0x + p1x) / 2;
    let m01y = (p0y + p1y) / 2;
    let m12x = (p1x + p2x) / 2;
    let m12y = (p1y + p2y) / 2;
    let mx = (m01x + m12x) / 2;
    let my = (m01y + m12y) / 2;

    flatten_bezier_quad(p0x, p0y, m01x, m01y, mx, my, output, depth + 1);
    flatten_bezier_quad(mx, my, m12x, m12y, p2x, p2y, output, depth + 1);
}

/// Flatten a cubic Bezier curve (OpenType CFF) into line segments.
fn flatten_bezier_cubic(
    p0x: i32,
    p0y: i32,
    p1x: i32,
    p1y: i32,
    p2x: i32,
    p2y: i32,
    p3x: i32,
    p3y: i32,
    output: &mut Vec<(i32, i32)>,
    depth: u32,
) {
    // Flatness test: check deviation of control points from the baseline
    let dx = p3x - p0x;
    let dy = p3y - p0y;

    let d1 = ((p1x - p3x) as i64 * dy as i64 - (p1y - p3y) as i64 * dx as i64).abs();
    let d2 = ((p2x - p3x) as i64 * dy as i64 - (p2y - p3y) as i64 * dx as i64).abs();

    let flatness = d1 + d2;
    let threshold: i64 = 512;

    if flatness <= threshold || depth >= MAX_BEZIER_DEPTH {
        output.push((p3x, p3y));
        return;
    }

    // De Casteljau subdivision at t=0.5
    let m01x = (p0x + p1x) / 2;
    let m01y = (p0y + p1y) / 2;
    let m12x = (p1x + p2x) / 2;
    let m12y = (p1y + p2y) / 2;
    let m23x = (p2x + p3x) / 2;
    let m23y = (p2y + p3y) / 2;

    let m012x = (m01x + m12x) / 2;
    let m012y = (m01y + m12y) / 2;
    let m123x = (m12x + m23x) / 2;
    let m123y = (m12y + m23y) / 2;

    let mx = (m012x + m123x) / 2;
    let my = (m012y + m123y) / 2;

    flatten_bezier_cubic(
        p0x,
        p0y,
        m01x,
        m01y,
        m012x,
        m012y,
        mx,
        my,
        output,
        depth + 1,
    );
    flatten_bezier_cubic(
        mx,
        my,
        m123x,
        m123y,
        m23x,
        m23y,
        p3x,
        p3y,
        output,
        depth + 1,
    );
}

// ---------------------------------------------------------------------------
// FontFace implementation
// ---------------------------------------------------------------------------

impl FontFace {
    /// Create a new font face with the given metadata.
    pub fn new(
        id: u32,
        family_hash: u64,
        style: FontStyle,
        weight: u16,
        units_per_em: u16,
        ascender: i16,
        descender: i16,
        line_gap: i16,
    ) -> Self {
        let size_px = DEFAULT_SIZE_PX;
        let scale_q16 = if units_per_em > 0 {
            ((size_px as i64 * Q16_ONE as i64) / units_per_em as i64) as i32
        } else {
            Q16_ONE
        };

        FontFace {
            id,
            family_hash,
            style,
            weight,
            glyph_count: 0,
            units_per_em,
            ascender,
            descender,
            line_gap,
            size_px,
            scale_q16,
            glyphs: Vec::new(),
            kern_pairs: Vec::new(),
        }
    }

    /// Recalculate the scale factor after size change.
    fn recalculate_scale(&mut self) {
        if self.units_per_em > 0 {
            self.scale_q16 =
                ((self.size_px as i64 * Q16_ONE as i64) / self.units_per_em as i64) as i32;
        }
    }

    /// Set the rendering size in pixels and update the scale factor.
    pub fn set_size(&mut self, size_px: u32) {
        self.size_px = if size_px == 0 { 1 } else { size_px };
        self.recalculate_scale();
    }

    /// Look up a glyph by codepoint. Returns index into glyphs vec.
    pub fn find_glyph(&self, codepoint: u32) -> Option<usize> {
        for (i, g) in self.glyphs.iter().enumerate() {
            if g.codepoint == codepoint {
                return Some(i);
            }
        }
        None
    }

    /// Get the kerning adjustment (in font units) for a pair of codepoints.
    pub fn kern_pair(&self, left: u32, right: u32) -> i16 {
        for &(l, r, adj) in &self.kern_pairs {
            if l == left && r == right {
                return adj;
            }
        }
        0
    }

    /// Get line metrics at the current size (all values Q16 pixels).
    pub fn get_line_metrics(&self) -> LineMetrics {
        let asc = q16_mul(self.ascender as i32 * Q16_ONE / 1, self.scale_q16);
        let desc = q16_mul(self.descender as i32 * Q16_ONE / 1, self.scale_q16);
        let gap = q16_mul(self.line_gap as i32 * Q16_ONE / 1, self.scale_q16);
        let height = asc - desc + gap;

        LineMetrics {
            ascender_q16: asc,
            descender_q16: desc,
            line_gap_q16: gap,
            line_height_q16: height,
        }
    }
}

// ---------------------------------------------------------------------------
// Glyph / contour helpers
// ---------------------------------------------------------------------------

impl Glyph {
    /// Create a new empty glyph.
    pub fn new(codepoint: u32, advance_width: i16, lsb: i16) -> Self {
        Glyph {
            codepoint,
            advance_width,
            lsb,
            contours: Vec::new(),
        }
    }

    /// Flatten all contours into a list of line segments (polylines).
    /// Each inner Vec is one closed contour of (x, y) points in font units.
    pub fn flatten_contours(&self) -> Vec<Vec<(i32, i32)>> {
        let mut result = Vec::new();

        for contour in &self.contours {
            let pts = &contour.points;
            let on = &contour.on_curve;
            let n = pts.len();
            if n < 2 {
                continue;
            }

            let mut flattened: Vec<(i32, i32)> = Vec::new();
            flattened.push(pts[0]);

            let mut i = 0;
            while i < n {
                let next = (i + 1) % n;
                let next2 = (i + 2) % n;

                if on[i] && i + 1 < n && !on[next] && i + 2 <= n {
                    // Quadratic Bezier: on-curve, off-curve, on-curve
                    let p0 = pts[i];
                    let p1 = pts[next];
                    let p2 = if on[next2] {
                        pts[next2]
                    } else {
                        // Implied on-curve point between two off-curve points
                        (
                            (pts[next].0 + pts[next2].0) / 2,
                            (pts[next].1 + pts[next2].1) / 2,
                        )
                    };

                    flatten_bezier_quad(p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, &mut flattened, 0);

                    i = if on[next2] { next2 } else { next };
                } else if on[i] && i + 1 < n && on[next] {
                    // Line segment
                    flattened.push(pts[next]);
                    i += 1;
                } else {
                    i += 1;
                }
            }

            // Close the contour
            if let (Some(&first), Some(&last)) = (flattened.first(), flattened.last()) {
                if first != last {
                    flattened.push(first);
                }
            }

            result.push(flattened);
        }

        result
    }
}

// ---------------------------------------------------------------------------
// Scanline rasterizer
// ---------------------------------------------------------------------------

/// Rasterize a glyph into a grayscale bitmap using scanline fill.
/// Uses the even-odd rule for winding.
fn rasterize_contours(
    contours: &[Vec<(i32, i32)>],
    scale_q16: i32,
    bearing_x: i32,
    bearing_y: i32,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut bitmap = vec![0u8; w * h];

    if w == 0 || h == 0 {
        return bitmap;
    }

    // For each scanline (row), find intersections with all edges
    for row in 0..h {
        let scan_y = row as i32;
        let mut intersections: Vec<i32> = Vec::new();

        for contour in contours {
            let n = contour.len();
            if n < 2 {
                continue;
            }
            for i in 0..n - 1 {
                let (mut x0, mut y0) = contour[i];
                let (mut x1, mut y1) = contour[i + 1];

                // Scale from font units to pixel coordinates
                x0 = q16_mul(x0, scale_q16) / Q16_ONE - bearing_x;
                y0 = bearing_y - q16_mul(y0, scale_q16) / Q16_ONE;
                x1 = q16_mul(x1, scale_q16) / Q16_ONE - bearing_x;
                y1 = bearing_y - q16_mul(y1, scale_q16) / Q16_ONE;

                // Skip horizontal edges
                if y0 == y1 {
                    continue;
                }

                // Ensure y0 <= y1
                if y0 > y1 {
                    let tmp = x0;
                    x0 = x1;
                    x1 = tmp;
                    let tmp = y0;
                    y0 = y1;
                    y1 = tmp;
                }

                // Check if scanline intersects this edge
                if scan_y < y0 || scan_y >= y1 {
                    continue;
                }

                // Compute X intersection using integer math
                let dy = y1 - y0;
                let dx = x1 - x0;
                let t_num = scan_y - y0;
                let ix = x0 + (dx * t_num) / dy;
                intersections.push(ix);
            }
        }

        // Sort intersections
        intersections.sort();

        // Fill between pairs (even-odd rule)
        let mut j = 0;
        while j + 1 < intersections.len() {
            let left = if intersections[j] < 0 {
                0
            } else {
                intersections[j] as usize
            };
            let right = if intersections[j + 1] < 0 {
                0
            } else {
                (intersections[j + 1] as usize).min(w)
            };

            let left = left.min(w);
            for col in left..right {
                let idx = row * w + col;
                if idx < bitmap.len() {
                    bitmap[idx] = 255;
                }
            }
            j += 2;
        }
    }

    bitmap
}

// ---------------------------------------------------------------------------
// FontEngine implementation
// ---------------------------------------------------------------------------

impl FontEngine {
    /// Create a new empty font engine.
    pub fn new() -> Self {
        FontEngine {
            fonts: Vec::new(),
            next_font_id: 1,
            active_font: 0,
            glyph_cache: Vec::new(),
            access_counter: 0,
        }
    }

    /// Load a font face from raw table data.
    /// In a real implementation this would parse TrueType/OpenType tables.
    /// Here we create a font face and populate built-in ASCII glyphs.
    pub fn load_font(&mut self, family_name: &[u8], style: FontStyle, weight: u16) -> Option<u32> {
        if self.fonts.len() >= MAX_FONTS {
            serial_println!(
                "[FONT] Cannot load font: maximum {} fonts reached",
                MAX_FONTS
            );
            return None;
        }

        let family_hash = fnv1a_hash(family_name);
        let id = self.next_font_id;
        self.next_font_id = self.next_font_id.saturating_add(1);

        let mut face = FontFace::new(
            id,
            family_hash,
            style,
            weight,
            2048, // units_per_em (standard TrueType value)
            1900, // ascender
            -500, // descender
            0,    // line_gap
        );

        // Generate built-in monospace glyphs for ASCII 32-126
        Self::populate_builtin_glyphs(&mut face);

        serial_println!(
            "[FONT] Loaded font id={} family_hash={:#018X} style={:?} weight={} glyphs={}",
            face.id,
            face.family_hash,
            face.style,
            face.weight,
            face.glyph_count
        );

        self.fonts.push(face);
        Some(id)
    }

    /// Populate a font face with basic built-in ASCII glyph outlines.
    /// These are simplified rectangular glyphs for fallback rendering.
    fn populate_builtin_glyphs(face: &mut FontFace) {
        let em = face.units_per_em as i32;
        let advance = (em * 6) / 10; // 60% of em for monospace
        let cap_height = (em * 7) / 10;
        let x_height = (em * 5) / 10;

        // Space (U+0020)
        let space = Glyph::new(0x0020, advance as i16, 0);
        face.glyphs.push(space);

        // Generate simple rectangular outlines for printable ASCII
        let mut cp: u32 = 0x0021;
        while cp <= 0x007E {
            let glyph_height = if cp >= 0x0041 && cp <= 0x005A {
                cap_height // uppercase
            } else if cp >= 0x0061 && cp <= 0x007A {
                x_height // lowercase
            } else {
                (cap_height + x_height) / 2 // symbols/digits
            };

            let width = (advance * 8) / 10;
            let lsb = (advance - width) / 2;

            let mut glyph = Glyph::new(cp, advance as i16, lsb as i16);

            // Create a simple rectangular contour
            let contour = GlyphContour {
                points: vec![
                    (lsb, 0),
                    (lsb + width, 0),
                    (lsb + width, glyph_height),
                    (lsb, glyph_height),
                ],
                on_curve: vec![true, true, true, true],
            };
            glyph.contours.push(contour);
            face.glyphs.push(glyph);

            cp += 1;
        }

        face.glyph_count = face.glyphs.len() as u32;

        // Add some basic kerning pairs
        // A-V, A-W, T-o, etc.
        face.kern_pairs.push((0x0041, 0x0056, -80)); // AV
        face.kern_pairs.push((0x0041, 0x0057, -60)); // AW
        face.kern_pairs.push((0x0054, 0x006F, -100)); // To
        face.kern_pairs.push((0x0056, 0x0041, -80)); // VA
        face.kern_pairs.push((0x0057, 0x0041, -60)); // WA
        face.kern_pairs.push((0x0054, 0x0061, -100)); // Ta
        face.kern_pairs.push((0x0046, 0x002E, -80)); // F.
        face.kern_pairs.push((0x004C, 0x0054, -80)); // LT
        face.kern_pairs.push((0x0050, 0x002E, -100)); // P.
        face.kern_pairs.push((0x0059, 0x006F, -80)); // Yo
    }

    /// Get the currently active font face.
    pub fn active_face(&self) -> Option<&FontFace> {
        self.fonts.get(self.active_font)
    }

    /// Get a mutable reference to the currently active font face.
    pub fn active_face_mut(&mut self) -> Option<&mut FontFace> {
        self.fonts.get_mut(self.active_font)
    }

    /// Set the active font by ID. Returns true if found.
    pub fn set_active_font(&mut self, font_id: u32) -> bool {
        for (i, f) in self.fonts.iter().enumerate() {
            if f.id == font_id {
                self.active_font = i;
                return true;
            }
        }
        false
    }

    /// Get a glyph from the active font by codepoint.
    pub fn get_glyph(&self, codepoint: u32) -> Option<&Glyph> {
        if let Some(face) = self.active_face() {
            if let Some(idx) = face.find_glyph(codepoint) {
                return Some(&face.glyphs[idx]);
            }
        }
        None
    }

    /// Rasterize a glyph at the current font size.
    /// Returns a grayscale bitmap or None if the glyph is not found.
    pub fn rasterize_glyph(&mut self, codepoint: u32) -> Option<RasterGlyph> {
        let face = self.fonts.get(self.active_font)?;
        let glyph_idx = face.find_glyph(codepoint)?;
        let glyph = &face.glyphs[glyph_idx];
        let scale = face.scale_q16;
        let font_id = face.id;
        let size_px = face.size_px;

        // Check cache first
        self.access_counter = self.access_counter.saturating_add(1);
        let ac = self.access_counter;
        for entry in self.glyph_cache.iter_mut() {
            if entry.font_id == font_id && entry.size_px == size_px && entry.codepoint == codepoint
            {
                entry.access_count = ac;
                return Some(entry.raster.clone());
            }
        }

        // Flatten contours
        let contours = glyph.flatten_contours();

        // Compute bounding box in scaled pixel coordinates
        let mut min_x: i32 = i32::MAX;
        let mut min_y: i32 = i32::MAX;
        let mut max_x: i32 = i32::MIN;
        let mut max_y: i32 = i32::MIN;

        for contour in &contours {
            for &(px, py) in contour {
                let sx = q16_mul(px, scale) / Q16_ONE;
                let sy = q16_mul(py, scale) / Q16_ONE;
                if sx < min_x {
                    min_x = sx;
                }
                if sy < min_y {
                    min_y = sy;
                }
                if sx > max_x {
                    max_x = sx;
                }
                if sy > max_y {
                    max_y = sy;
                }
            }
        }

        if min_x >= max_x || min_y >= max_y {
            // Degenerate or empty glyph (e.g., space)
            let adv = q16_mul(glyph.advance_width as i32, scale);
            return Some(RasterGlyph {
                codepoint,
                width: 0,
                height: 0,
                bearing_x: 0,
                bearing_y: 0,
                advance_q16: adv,
                bitmap: Vec::new(),
            });
        }

        let width = (max_x - min_x + 1) as u32;
        let height = (max_y - min_y + 1) as u32;
        let bearing_x = min_x;
        let bearing_y = max_y; // top of glyph relative to baseline

        let bitmap = rasterize_contours(&contours, scale, bearing_x, bearing_y, width, height);

        let adv = q16_mul(glyph.advance_width as i32, scale);

        let raster = RasterGlyph {
            codepoint,
            width,
            height,
            bearing_x,
            bearing_y,
            advance_q16: adv,
            bitmap,
        };

        // Cache the result
        if self.glyph_cache.len() >= MAX_GLYPH_CACHE {
            self.evict_glyph_cache();
        }
        self.glyph_cache.push(GlyphCacheEntry {
            font_id,
            size_px,
            codepoint,
            raster: raster.clone(),
            access_count: ac,
        });

        Some(raster)
    }

    /// Evict the least recently used entry from the glyph cache.
    fn evict_glyph_cache(&mut self) {
        if self.glyph_cache.is_empty() {
            return;
        }
        let mut min_ac = u64::MAX;
        let mut min_idx = 0;
        for (i, entry) in self.glyph_cache.iter().enumerate() {
            if entry.access_count < min_ac {
                min_ac = entry.access_count;
                min_idx = i;
            }
        }
        self.glyph_cache.remove(min_idx);
    }

    /// Shape a sequence of codepoints into positioned glyphs.
    /// Applies kerning and returns a list of ShapedGlyph entries.
    pub fn shape_text(&self, codepoints: &[u32]) -> Vec<ShapedGlyph> {
        let face = match self.active_face() {
            Some(f) => f,
            None => return Vec::new(),
        };

        let mut shaped = Vec::new();
        let mut cursor_x: i32 = 0;

        for (i, &cp) in codepoints.iter().enumerate() {
            let advance = if let Some(idx) = face.find_glyph(cp) {
                let g = &face.glyphs[idx];
                q16_mul(g.advance_width as i32, face.scale_q16)
            } else {
                // Fallback: half em advance
                q16_mul(face.units_per_em as i32 / 2, face.scale_q16)
            };

            // Apply kerning with the next codepoint
            let kern = if i + 1 < codepoints.len() {
                let next_cp = codepoints[i + 1];
                let kern_fu = face.kern_pair(cp, next_cp);
                q16_mul(kern_fu as i32, face.scale_q16)
            } else {
                0
            };

            shaped.push(ShapedGlyph {
                codepoint: cp,
                x_offset_q16: cursor_x,
                y_offset_q16: 0,
                advance_q16: advance,
            });

            cursor_x += advance + kern;
        }

        shaped
    }

    /// Measure a sequence of codepoints. Returns total width, height, and glyph count.
    pub fn measure_text(&self, codepoints: &[u32]) -> TextMetrics {
        let face = match self.active_face() {
            Some(f) => f,
            None => {
                return TextMetrics {
                    width_q16: 0,
                    height_q16: 0,
                    glyph_count: 0,
                };
            }
        };

        let shaped = self.shape_text(codepoints);
        let total_width = if let Some(last) = shaped.last() {
            last.x_offset_q16 + last.advance_q16
        } else {
            0
        };

        let lm = face.get_line_metrics();

        TextMetrics {
            width_q16: total_width,
            height_q16: lm.line_height_q16,
            glyph_count: shaped.len() as u32,
        }
    }

    /// Get the kerning value between two codepoints for the active font.
    /// Returns the adjustment in Q16 pixels.
    pub fn kern_pair(&self, left: u32, right: u32) -> i32 {
        if let Some(face) = self.active_face() {
            let kern_fu = face.kern_pair(left, right);
            q16_mul(kern_fu as i32, face.scale_q16)
        } else {
            0
        }
    }

    /// Get line metrics for the active font at its current size.
    pub fn get_line_metrics(&self) -> Option<LineMetrics> {
        self.active_face().map(|f| f.get_line_metrics())
    }

    /// Set the rendering size for the active font.
    pub fn set_size(&mut self, size_px: u32) {
        if let Some(face) = self.active_face_mut() {
            face.set_size(size_px);
            serial_println!(
                "[FONT] Set font id={} size={}px scale_q16={}",
                face.id,
                face.size_px,
                face.scale_q16
            );
        }
    }

    /// Get a list of all loaded font IDs and family hashes.
    pub fn get_font_list(&self) -> Vec<(u32, u64, FontStyle, u16)> {
        let mut list = Vec::new();
        for f in &self.fonts {
            list.push((f.id, f.family_hash, f.style, f.weight));
        }
        list
    }

    /// Remove a font by ID. Returns true if found and removed.
    pub fn unload_font(&mut self, font_id: u32) -> bool {
        let before = self.fonts.len();
        self.fonts.retain(|f| f.id != font_id);
        // Also purge cached glyphs for this font
        self.glyph_cache.retain(|e| e.font_id != font_id);
        let removed = before != self.fonts.len();
        if removed {
            // Fix active font index if needed
            if self.active_font >= self.fonts.len() && !self.fonts.is_empty() {
                self.active_font = 0;
            }
            serial_println!("[FONT] Unloaded font id={}", font_id);
        }
        removed
    }

    /// Clear the entire rasterized glyph cache.
    pub fn clear_cache(&mut self) {
        let count = self.glyph_cache.len();
        self.glyph_cache.clear();
        self.access_counter = 0;
        serial_println!("[FONT] Cleared glyph cache ({} entries)", count);
    }
}

// ---------------------------------------------------------------------------
// Module initialization
// ---------------------------------------------------------------------------

/// Initialize the font engine subsystem.
pub fn init() {
    let mut engine = FontEngine::new();

    // Load a default system font
    let _default_id = engine.load_font(b"Genesis Sans", FontStyle::Regular, 400);

    *FONT_ENGINE.lock() = Some(engine);
    serial_println!(
        "[FONT] Font engine initialized (max_fonts={}, glyph_cache={}, default_size={}px)",
        MAX_FONTS,
        MAX_GLYPH_CACHE,
        DEFAULT_SIZE_PX
    );
}

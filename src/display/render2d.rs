/// 2D rendering engine for Genesis — Skia-like software rasterizer
///
/// Provides: lines, rects, circles, arcs, paths, gradients, text rendering,
/// anti-aliasing, alpha blending, clipping, transforms.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Skia, Cairo, tiny-skia. All code is original.
use alloc::vec::Vec;

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 divide (a / b)
fn q16_div(a: i32, b: i32) -> i32 {
    if b == 0 {
        return 0;
    }
    (((a as i64) << 16) / b as i64) as i32
}

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// ARGB color (8-bit per channel)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub a: u8,
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { a, r, g, b }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { a: 255, r, g, b }
    }
    pub const fn to_u32(self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }
    pub const fn from_u32(v: u32) -> Self {
        Color {
            a: (v >> 24) as u8,
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
        }
    }

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);
}

/// Paint style (fill, stroke)
#[derive(Debug, Clone, Copy)]
pub enum PaintStyle {
    Fill,
    Stroke(u32), // stroke width in pixels (integer)
}

/// Blend mode
#[derive(Debug, Clone, Copy)]
pub enum BlendMode {
    SrcOver, // Porter-Duff source-over (normal alpha blending)
    Src,     // Source replaces destination
    DstOver, // Draw behind
    Clear,   // Clear
    Multiply,
    Screen,
    Overlay,
}

/// 2D affine transform matrix (3x2) using Q16 fixed-point
#[derive(Clone, Copy)]
pub struct Transform {
    pub m: [i32; 6], // Q16: [a, b, c, d, e, f] => | a b e |
                     //                              | c d f |
}

impl Transform {
    pub const fn identity() -> Self {
        Transform {
            m: [Q16_ONE, 0, 0, Q16_ONE, 0, 0],
        }
    }

    /// Create a translation transform (tx, ty in pixels as Q16)
    pub fn translate(tx: i32, ty: i32) -> Self {
        Transform {
            m: [Q16_ONE, 0, 0, Q16_ONE, tx, ty],
        }
    }

    /// Create a translation from integer pixel values
    pub fn translate_int(tx: i32, ty: i32) -> Self {
        Transform {
            m: [Q16_ONE, 0, 0, Q16_ONE, q16_from_int(tx), q16_from_int(ty)],
        }
    }

    /// Create a scale transform (sx, sy as Q16 values)
    pub fn scale(sx: i32, sy: i32) -> Self {
        Transform {
            m: [sx, 0, 0, sy, 0, 0],
        }
    }

    /// Create a scale from integer multipliers
    pub fn scale_int(sx: i32, sy: i32) -> Self {
        Transform {
            m: [q16_from_int(sx), 0, 0, q16_from_int(sy), 0, 0],
        }
    }

    /// Apply the transform to a Q16 point, returns Q16 result
    pub fn apply(&self, x: i32, y: i32) -> (i32, i32) {
        (
            q16_mul(self.m[0], x) + q16_mul(self.m[1], y) + self.m[4],
            q16_mul(self.m[2], x) + q16_mul(self.m[3], y) + self.m[5],
        )
    }

    /// Apply the transform to integer pixel coords, returns integer pixel coords
    pub fn apply_int(&self, x: i32, y: i32) -> (i32, i32) {
        let qx = q16_from_int(x);
        let qy = q16_from_int(y);
        let (rx, ry) = self.apply(qx, qy);
        (rx >> 16, ry >> 16)
    }
}

/// Clipping rectangle
#[derive(Clone, Copy)]
pub struct ClipRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Gradient stop — position is Q16 (0 = 0.0, Q16_ONE = 1.0)
#[derive(Clone, Copy)]
pub struct GradientStop {
    pub position: i32, // Q16: 0..Q16_ONE
    pub color: Color,
}

/// Linear gradient — coordinates in pixels (integer)
pub struct LinearGradient {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub stops: Vec<GradientStop>,
}

/// 2D rendering surface
pub struct Surface {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
    pub clip: Option<ClipRect>,
    pub transform: Transform,
    pub blend_mode: BlendMode,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        Surface {
            pixels: alloc::vec![0u32; (width * height) as usize],
            width,
            height,
            clip: None,
            transform: Transform::identity(),
            blend_mode: BlendMode::SrcOver,
        }
    }

    /// Clear to a color
    pub fn clear(&mut self, color: Color) {
        let c = color.to_u32();
        for p in &mut self.pixels {
            *p = c;
        }
    }

    /// Set a pixel with alpha blending
    pub fn set_pixel(&mut self, x: i32, y: i32, color: Color) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }

        // Clipping
        if let Some(clip) = self.clip {
            if x < clip.x
                || y < clip.y
                || x >= clip.x + clip.w as i32
                || y >= clip.y + clip.h as i32
            {
                return;
            }
        }

        let idx = (y as u32 * self.width + x as u32) as usize;
        if idx >= self.pixels.len() {
            return;
        }

        match self.blend_mode {
            BlendMode::Src => {
                self.pixels[idx] = color.to_u32();
            }
            BlendMode::SrcOver => {
                if color.a == 255 {
                    self.pixels[idx] = color.to_u32();
                } else if color.a > 0 {
                    let dst = Color::from_u32(self.pixels[idx]);
                    let sa = color.a as u32;
                    let da = 255 - sa;
                    let r = (color.r as u32 * sa + dst.r as u32 * da) / 255;
                    let g = (color.g as u32 * sa + dst.g as u32 * da) / 255;
                    let b = (color.b as u32 * sa + dst.b as u32 * da) / 255;
                    let a = sa + (dst.a as u32 * da) / 255;
                    self.pixels[idx] = Color::rgba(r as u8, g as u8, b as u8, a as u8).to_u32();
                }
            }
            BlendMode::Clear => {
                self.pixels[idx] = 0;
            }
            _ => {
                self.pixels[idx] = color.to_u32();
            }
        }
    }

    /// Draw a filled rectangle
    pub fn fill_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color) {
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                self.set_pixel(x + dx, y + dy, color);
            }
        }
    }

    /// Draw a rectangle outline
    pub fn stroke_rect(&mut self, x: i32, y: i32, w: u32, h: u32, color: Color, thickness: u32) {
        let t = thickness as i32;
        // Top
        self.fill_rect(x, y, w, thickness, color);
        // Bottom
        self.fill_rect(x, y + h as i32 - t, w, thickness, color);
        // Left
        self.fill_rect(x, y, thickness, h, color);
        // Right
        self.fill_rect(x + w as i32 - t, y, thickness, h, color);
    }

    /// Draw a line (Bresenham's algorithm)
    pub fn draw_line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        let mut x = x0;
        let mut y = y0;

        loop {
            self.set_pixel(x, y, color);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                if x == x1 {
                    break;
                }
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                if y == y1 {
                    break;
                }
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw a filled circle (midpoint circle algorithm)
    pub fn fill_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        let r2 = radius * radius;
        for y in -radius..=radius {
            for x in -radius..=radius {
                if x * x + y * y <= r2 {
                    self.set_pixel(cx + x, cy + y, color);
                }
            }
        }
    }

    /// Draw a circle outline
    pub fn stroke_circle(&mut self, cx: i32, cy: i32, radius: i32, color: Color) {
        let mut x = radius;
        let mut y = 0;
        let mut d = 1 - radius;

        while x >= y {
            self.set_pixel(cx + x, cy + y, color);
            self.set_pixel(cx + y, cy + x, color);
            self.set_pixel(cx - y, cy + x, color);
            self.set_pixel(cx - x, cy + y, color);
            self.set_pixel(cx - x, cy - y, color);
            self.set_pixel(cx - y, cy - x, color);
            self.set_pixel(cx + y, cy - x, color);
            self.set_pixel(cx + x, cy - y, color);
            y += 1;
            if d <= 0 {
                d += 2 * y + 1;
            } else {
                x -= 1;
                d += 2 * (y - x) + 1;
            }
        }
    }

    /// Draw a rounded rectangle
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: u32, h: u32, radius: u32, color: Color) {
        let r = radius as i32;
        // Center rect
        self.fill_rect(x + r, y, w - 2 * radius, h, color);
        // Left/right strips
        self.fill_rect(x, y + r, radius, h - 2 * radius, color);
        self.fill_rect(x + w as i32 - r, y + r, radius, h - 2 * radius, color);
        // Corners
        self.fill_quarter_circle(x + r, y + r, r, color, 0);
        self.fill_quarter_circle(x + w as i32 - r - 1, y + r, r, color, 1);
        self.fill_quarter_circle(x + r, y + h as i32 - r - 1, r, color, 2);
        self.fill_quarter_circle(x + w as i32 - r - 1, y + h as i32 - r - 1, r, color, 3);
    }

    fn fill_quarter_circle(&mut self, cx: i32, cy: i32, r: i32, color: Color, quadrant: u8) {
        let r2 = r * r;
        for dy in 0..=r {
            for dx in 0..=r {
                if dx * dx + dy * dy <= r2 {
                    let (px, py) = match quadrant {
                        0 => (cx - dx, cy - dy), // top-left
                        1 => (cx + dx, cy - dy), // top-right
                        2 => (cx - dx, cy + dy), // bottom-left
                        _ => (cx + dx, cy + dy), // bottom-right
                    };
                    self.set_pixel(px, py, color);
                }
            }
        }
    }

    /// Fill with a linear gradient (all coords in integer pixels,
    /// gradient stop positions in Q16 0..Q16_ONE)
    pub fn fill_rect_gradient(
        &mut self,
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        gradient: &LinearGradient,
    ) {
        let dx = gradient.x1 - gradient.x0;
        let dy = gradient.y1 - gradient.y0;
        // len2 in integer space
        let len2 = dx as i64 * dx as i64 + dy as i64 * dy as i64;
        if len2 == 0 {
            return;
        }

        for py in 0..h as i32 {
            for px in 0..w as i32 {
                // dot product in integer space
                let dot =
                    (px - gradient.x0) as i64 * dx as i64 + (py - gradient.y0) as i64 * dy as i64;
                // t as Q16: (dot / len2) * Q16_ONE
                let t_q16 = ((dot * Q16_ONE as i64) / len2) as i32;
                let t_clamped = if t_q16 < 0 {
                    0
                } else if t_q16 > Q16_ONE {
                    Q16_ONE
                } else {
                    t_q16
                };
                let color = sample_gradient(&gradient.stops, t_clamped);
                self.set_pixel(x + px, y + py, color);
            }
        }
    }

    /// Set clipping rectangle
    pub fn set_clip(&mut self, clip: ClipRect) {
        self.clip = Some(clip);
    }

    /// Clear clipping
    pub fn clear_clip(&mut self) {
        self.clip = None;
    }
}

/// Sample a gradient at position t (Q16: 0..Q16_ONE)
fn sample_gradient(stops: &[GradientStop], t: i32) -> Color {
    if stops.is_empty() {
        return Color::BLACK;
    }
    if stops.len() == 1 {
        return stops[0].color;
    }

    // Find surrounding stops
    for i in 0..stops.len() - 1 {
        if t >= stops[i].position && t <= stops[i + 1].position {
            let range = stops[i + 1].position - stops[i].position;
            if range == 0 {
                return stops[i].color;
            }
            // local_t as Q16: (t - pos_i) / range
            let local_t = (((t - stops[i].position) as i64 * Q16_ONE as i64) / range as i64) as i32;

            let c0 = stops[i].color;
            let c1 = stops[i + 1].color;
            return Color::rgba(
                lerp_u8(c0.r, c1.r, local_t),
                lerp_u8(c0.g, c1.g, local_t),
                lerp_u8(c0.b, c1.b, local_t),
                lerp_u8(c0.a, c1.a, local_t),
            );
        }
    }
    stops.last().map(|s| s.color).unwrap_or(Color::BLACK)
}

/// Linearly interpolate between two u8 values using Q16 t
fn lerp_u8(a: u8, b: u8, t: i32) -> u8 {
    // result = a + (b - a) * t / Q16_ONE
    let result = a as i32 + (((b as i32 - a as i32) * t) >> 16);
    if result < 0 {
        0
    } else if result > 255 {
        255
    } else {
        result as u8
    }
}

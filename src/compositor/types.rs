// compositor/types.rs - Core type definitions for the compositor

use core::fmt;

/// RGBA color representation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    /// Blend this color over another using alpha compositing
    pub fn blend_over(&self, dst: Color) -> Color {
        if self.a == 255 {
            return *self;
        }
        if self.a == 0 {
            return dst;
        }

        let src_alpha = self.a as u32;
        let dst_alpha = dst.a as u32;
        let inv_alpha = 255 - src_alpha;

        let out_alpha = src_alpha + (dst_alpha * inv_alpha) / 255;

        if out_alpha == 0 {
            return Color::transparent();
        }

        let r = ((self.r as u32 * src_alpha) + (dst.r as u32 * dst_alpha * inv_alpha / 255)) / out_alpha;
        let g = ((self.g as u32 * src_alpha) + (dst.g as u32 * dst_alpha * inv_alpha / 255)) / out_alpha;
        let b = ((self.b as u32 * src_alpha) + (dst.b as u32 * dst_alpha * inv_alpha / 255)) / out_alpha;

        Color::new(r as u8, g as u8, b as u8, out_alpha as u8)
    }
}

/// 2D rectangle
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        let x_overlap = self.x < other.x + other.width as i32 &&
                        self.x + self.width as i32 > other.x;
        let y_overlap = self.y < other.y + other.height as i32 &&
                        self.y + self.height as i32 > other.y;
        x_overlap && y_overlap
    }

    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        if !self.intersects(other) {
            return None;
        }

        let x1 = self.x.max(other.x);
        let y1 = self.y.max(other.y);
        let x2 = (self.x + self.width as i32).min(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).min(other.y + other.height as i32);

        Some(Rect::new(x1, y1, (x2 - x1) as u32, (y2 - y1) as u32))
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let x1 = self.x.min(other.x);
        let y1 = self.y.min(other.y);
        let x2 = (self.x + self.width as i32).max(other.x + other.width as i32);
        let y2 = (self.y + self.height as i32).max(other.y + other.height as i32);

        Rect::new(x1, y1, (x2 - x1) as u32, (y2 - y1) as u32)
    }

    pub fn contains_point(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.width as i32 &&
        y >= self.y && y < self.y + self.height as i32
    }
}

/// Dirty region tracker
#[derive(Debug, Clone)]
pub struct DirtyRegion {
    rects: [Option<Rect>; 16],
    count: usize,
}

impl DirtyRegion {
    pub fn new() -> Self {
        Self {
            rects: [None; 16],
            count: 0,
        }
    }

    pub fn add(&mut self, rect: Rect) {
        if self.count < 16 {
            self.rects[self.count] = Some(rect);
            self.count = self.count.saturating_add(1);
        } else {
            // Coalesce into bounding box if too many regions
            self.coalesce();
        }
    }

    pub fn rects(&self) -> &[Option<Rect>] {
        &self.rects[..self.count]
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn clear(&mut self) {
        self.count = 0;
    }

    fn coalesce(&mut self) {
        if self.count == 0 {
            return;
        }

        let mut bounds = self.rects[0].unwrap();
        for i in 1..self.count {
            if let Some(rect) = self.rects[i] {
                bounds = bounds.union(&rect);
            }
        }

        self.rects[0] = Some(bounds);
        self.count = 1;
    }
}

/// Pixel format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    RGBA8888,
    RGBX8888,
    RGB888,
    RGB565,
    BGRA8888,
}

impl PixelFormat {
    pub fn bytes_per_pixel(&self) -> usize {
        match self {
            PixelFormat::RGBA8888 => 4,
            PixelFormat::RGBX8888 => 4,
            PixelFormat::RGB888 => 3,
            PixelFormat::RGB565 => 2,
            PixelFormat::BGRA8888 => 4,
        }
    }
}

/// Blend mode for layer composition
///
/// All modes operate in integer arithmetic (no floats).
/// Porter-Duff SRC_OVER is the foundation; photographic modes
/// (Multiply, Screen, Overlay) are implemented with integer approximations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// Opaque copy — src fully replaces dst (alpha ignored).
    None,
    /// Source channels are premultiplied; scale by layer opacity then SRC_OVER.
    Premultiplied,
    /// Straight-alpha SRC_OVER: combined_alpha = src.a × layer_alpha / 255.
    Coverage,
    /// Photographic Multiply: dst × src / 255, then SRC_OVER.
    Multiply,
    /// Photographic Screen: 255 − (255−dst)×(255−src)/255, then SRC_OVER.
    Screen,
    /// Photographic Overlay: Multiply for dark pixels, Screen for bright ones.
    Overlay,
}

/// Transform/rotation flags
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transform {
    pub flip_h: bool,
    pub flip_v: bool,
    pub rotate_90: bool,
}

impl Transform {
    pub const IDENTITY: Transform = Transform {
        flip_h: false,
        flip_v: false,
        rotate_90: false,
    };

    pub fn rotate_180() -> Self {
        Self {
            flip_h: true,
            flip_v: true,
            rotate_90: false,
        }
    }

    pub fn rotate_270() -> Self {
        Self {
            flip_h: true,
            flip_v: false,
            rotate_90: true,
        }
    }
}

/// Composition type hint for hardware composer
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompositionType {
    Device,      // Hardware overlay
    SolidColor,  // Solid color - no buffer needed
    Cursor,      // Hardware cursor
    Client,      // GPU/CPU composition required
}

/// Buffer usage flags
#[derive(Debug, Clone, Copy)]
pub struct BufferUsage {
    pub cpu_read: bool,
    pub cpu_write: bool,
    pub gpu_render_target: bool,
    pub gpu_texture: bool,
    pub composer_overlay: bool,
    pub protected: bool,
}

impl BufferUsage {
    pub const fn default() -> Self {
        Self {
            cpu_read: false,
            cpu_write: true,
            gpu_render_target: false,
            gpu_texture: true,
            composer_overlay: true,
            protected: false,
        }
    }
}

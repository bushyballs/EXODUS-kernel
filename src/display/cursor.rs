use crate::sync::Mutex;
/// Hardware/software cursor rendering
///
/// Part of the AIOS display layer. Manages cursor shape, position,
/// animation frames, visibility, and rendering to the framebuffer.
/// Supports both hardware cursor planes and software-rendered cursors.
use alloc::vec::Vec;

/// Cursor shape types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    Arrow,
    IBeam,
    Hand,
    Resize,
    Wait,
    Hidden,
    Custom(u32), // custom bitmap cursor id
}

/// A single cursor bitmap frame (32x32 RGBA)
#[derive(Clone)]
pub struct CursorBitmap {
    pub width: u32,
    pub height: u32,
    pub hotspot_x: u32,
    pub hotspot_y: u32,
    pub pixels: Vec<u8>, // RGBA
}

impl CursorBitmap {
    /// Create a new cursor bitmap
    pub fn new(width: u32, height: u32, hotspot_x: u32, hotspot_y: u32) -> Self {
        let size = (width * height * 4) as usize;
        Self {
            width,
            height,
            hotspot_x,
            hotspot_y,
            pixels: Vec::with_capacity(size),
        }
    }

    /// Generate the default arrow cursor bitmap (32x32)
    fn generate_arrow() -> Self {
        let w: u32 = 32;
        let h: u32 = 32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                // Simple arrow shape: filled triangle
                let in_arrow = x <= y && x < 16 && y < 24;
                let on_border = (x == 0 && y < 24) || (x == y && x < 16) || (y == 23 && x < 16);
                if on_border {
                    // Black border
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(255);
                } else if in_arrow {
                    // White fill
                    pixels.push(255);
                    pixels.push(255);
                    pixels.push(255);
                    pixels.push(255);
                } else {
                    // Transparent
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                }
            }
        }
        Self {
            width: w,
            height: h,
            hotspot_x: 0,
            hotspot_y: 0,
            pixels,
        }
    }

    /// Generate an I-beam text cursor bitmap
    fn generate_ibeam() -> Self {
        let w: u32 = 16;
        let h: u32 = 32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let is_top_bar = y < 2 && x >= 4 && x <= 12;
                let is_bottom_bar = y >= 30 && x >= 4 && x <= 12;
                let is_stem = x >= 7 && x <= 8 && y >= 2 && y < 30;
                if is_top_bar || is_bottom_bar || is_stem {
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(255);
                } else {
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                }
            }
        }
        Self {
            width: w,
            height: h,
            hotspot_x: 8,
            hotspot_y: 16,
            pixels,
        }
    }

    /// Generate a hand/pointer cursor bitmap
    fn generate_hand() -> Self {
        let w: u32 = 32;
        let h: u32 = 32;
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                // Simplified pointing hand: finger on top, palm below
                let in_finger = x >= 10 && x <= 14 && y < 16;
                let in_palm = x >= 6 && x <= 22 && y >= 12 && y < 28;
                if in_finger || in_palm {
                    pixels.push(255);
                    pixels.push(255);
                    pixels.push(255);
                    pixels.push(255);
                } else {
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                    pixels.push(0);
                }
            }
        }
        Self {
            width: w,
            height: h,
            hotspot_x: 12,
            hotspot_y: 0,
            pixels,
        }
    }
}

/// Cursor animation state for animated cursors (e.g., wait spinner)
struct CursorAnimation {
    frames: Vec<CursorBitmap>,
    current_frame: usize,
    frame_duration_ms: u32,
    elapsed_ms: u32,
    looping: bool,
}

impl CursorAnimation {
    fn new(frame_duration_ms: u32) -> Self {
        Self {
            frames: Vec::new(),
            current_frame: 0,
            frame_duration_ms,
            elapsed_ms: 0,
            looping: true,
        }
    }

    fn add_frame(&mut self, frame: CursorBitmap) {
        self.frames.push(frame);
    }

    fn tick(&mut self, delta_ms: u32) -> bool {
        if self.frames.is_empty() {
            return false;
        }
        self.elapsed_ms += delta_ms;
        if self.elapsed_ms >= self.frame_duration_ms {
            self.elapsed_ms -= self.frame_duration_ms;
            let next = self.current_frame + 1;
            if next >= self.frames.len() {
                if self.looping {
                    self.current_frame = 0;
                }
            } else {
                self.current_frame = next;
            }
            return true; // frame changed
        }
        false
    }

    fn current_bitmap(&self) -> Option<&CursorBitmap> {
        self.frames.get(self.current_frame)
    }
}

/// Manages cursor rendering and position
pub struct CursorRenderer {
    pub x: i32,
    pub y: i32,
    pub shape: CursorShape,
    pub visible: bool,
    pub hw_cursor: bool,
    prev_x: i32,
    prev_y: i32,
    arrow_bitmap: CursorBitmap,
    ibeam_bitmap: CursorBitmap,
    hand_bitmap: CursorBitmap,
    custom_bitmaps: Vec<(u32, CursorBitmap)>,
    animation: Option<CursorAnimation>,
    screen_width: u32,
    screen_height: u32,
    trail_enabled: bool,
    trail_positions: Vec<(i32, i32)>,
    trail_max: usize,
}

impl CursorRenderer {
    pub fn new() -> Self {
        let arrow = CursorBitmap::generate_arrow();
        let ibeam = CursorBitmap::generate_ibeam();
        let hand = CursorBitmap::generate_hand();

        crate::serial_println!("[cursor] renderer created, default arrow cursor");
        Self {
            x: 0,
            y: 0,
            shape: CursorShape::Arrow,
            visible: true,
            hw_cursor: false,
            prev_x: 0,
            prev_y: 0,
            arrow_bitmap: arrow,
            ibeam_bitmap: ibeam,
            hand_bitmap: hand,
            custom_bitmaps: Vec::new(),
            animation: None,
            screen_width: 1920,
            screen_height: 1080,
            trail_enabled: false,
            trail_positions: Vec::new(),
            trail_max: 8,
        }
    }

    pub fn set_position(&mut self, x: i32, y: i32) {
        self.prev_x = self.x;
        self.prev_y = self.y;

        // Clamp to screen boundaries
        self.x = if x < 0 {
            0
        } else if x >= self.screen_width as i32 {
            self.screen_width as i32 - 1
        } else {
            x
        };
        self.y = if y < 0 {
            0
        } else if y >= self.screen_height as i32 {
            self.screen_height as i32 - 1
        } else {
            y
        };

        // Update trail
        if self.trail_enabled {
            self.trail_positions.push((self.prev_x, self.prev_y));
            if self.trail_positions.len() > self.trail_max {
                self.trail_positions.remove(0);
            }
        }
    }

    pub fn set_shape(&mut self, shape: CursorShape) {
        if self.shape == shape {
            return;
        }
        crate::serial_println!("[cursor] shape changed to {:?}", shape);

        // Stop any running animation when switching from Wait
        if !matches!(shape, CursorShape::Wait) {
            self.animation = None;
        }

        // Start wait animation if switching to Wait
        if matches!(shape, CursorShape::Wait) && self.animation.is_none() {
            let mut anim = CursorAnimation::new(150);
            // Generate 4 rotation frames of a simple spinner
            for rotation in 0..4u32 {
                let w: u32 = 16;
                let h: u32 = 16;
                let mut pixels = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let dx = x as i32 - 8;
                        let dy = y as i32 - 8;
                        let dist_sq = (dx * dx + dy * dy) as u32;
                        let in_ring = dist_sq >= 25 && dist_sq <= 64;
                        // Each rotation lights up a different quadrant
                        let quadrant = match (dx >= 0, dy >= 0) {
                            (true, false) => 0u32,
                            (true, true) => 1,
                            (false, true) => 2,
                            (false, false) => 3,
                        };
                        if in_ring && quadrant == rotation {
                            pixels.push(0);
                            pixels.push(0);
                            pixels.push(0);
                            pixels.push(255);
                        } else if in_ring {
                            pixels.push(128);
                            pixels.push(128);
                            pixels.push(128);
                            pixels.push(180);
                        } else {
                            pixels.push(0);
                            pixels.push(0);
                            pixels.push(0);
                            pixels.push(0);
                        }
                    }
                }
                anim.add_frame(CursorBitmap {
                    width: w,
                    height: h,
                    hotspot_x: 8,
                    hotspot_y: 8,
                    pixels,
                });
            }
            self.animation = Some(anim);
        }

        self.visible = !matches!(shape, CursorShape::Hidden);
        self.shape = shape;
    }

    /// Register a custom bitmap cursor with the given ID
    pub fn register_custom(&mut self, id: u32, bitmap: CursorBitmap) {
        // Replace if already exists
        for entry in self.custom_bitmaps.iter_mut() {
            if entry.0 == id {
                entry.1 = bitmap;
                return;
            }
        }
        self.custom_bitmaps.push((id, bitmap));
        crate::serial_println!("[cursor] registered custom bitmap id={}", id);
    }

    /// Get the active cursor bitmap for the current shape
    pub fn current_bitmap(&self) -> Option<&CursorBitmap> {
        // Check animation first (for Wait cursor)
        if let Some(ref anim) = self.animation {
            return anim.current_bitmap();
        }
        match self.shape {
            CursorShape::Arrow => Some(&self.arrow_bitmap),
            CursorShape::IBeam => Some(&self.ibeam_bitmap),
            CursorShape::Hand => Some(&self.hand_bitmap),
            CursorShape::Resize => Some(&self.arrow_bitmap), // fallback
            CursorShape::Wait => None,
            CursorShape::Hidden => None,
            CursorShape::Custom(id) => {
                for entry in &self.custom_bitmaps {
                    if entry.0 == id {
                        return Some(&entry.1);
                    }
                }
                None
            }
        }
    }

    /// Advance cursor animation by delta_ms. Returns true if frame changed.
    pub fn tick(&mut self, delta_ms: u32) -> bool {
        if let Some(ref mut anim) = self.animation {
            anim.tick(delta_ms)
        } else {
            false
        }
    }

    /// Set screen dimensions for clamping
    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
        crate::serial_println!("[cursor] screen size set to {}x{}", width, height);
    }

    /// Enable or disable cursor trail
    pub fn set_trail(&mut self, enabled: bool) {
        self.trail_enabled = enabled;
        if !enabled {
            self.trail_positions.clear();
        }
    }

    /// Get the dirty rectangle that needs redrawing after a cursor move
    pub fn dirty_rect(&self) -> (i32, i32, u32, u32) {
        let bmp_w = 32i32;
        let bmp_h = 32i32;
        let min_x = if self.prev_x < self.x {
            self.prev_x
        } else {
            self.x
        };
        let min_y = if self.prev_y < self.y {
            self.prev_y
        } else {
            self.y
        };
        let max_x = if self.prev_x > self.x {
            self.prev_x
        } else {
            self.x
        };
        let max_y = if self.prev_y > self.y {
            self.prev_y
        } else {
            self.y
        };
        (
            min_x,
            min_y,
            (max_x - min_x + bmp_w) as u32,
            (max_y - min_y + bmp_h) as u32,
        )
    }
}

static CURSOR: Mutex<Option<CursorRenderer>> = Mutex::new(None);

pub fn init() {
    let renderer = CursorRenderer::new();
    let mut cursor = CURSOR.lock();
    *cursor = Some(renderer);
    crate::serial_println!("[cursor] subsystem initialized");
}

/// Get cursor position
pub fn position() -> (i32, i32) {
    let cursor = CURSOR.lock();
    match cursor.as_ref() {
        Some(c) => (c.x, c.y),
        None => (0, 0),
    }
}

/// Move cursor to absolute position
pub fn move_to(x: i32, y: i32) {
    let mut cursor = CURSOR.lock();
    if let Some(ref mut c) = *cursor {
        c.set_position(x, y);
    }
}

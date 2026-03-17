use crate::sync::Mutex;

/// Color filter mode for vision accessibility
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFilter {
    None,
    Grayscale,
    Deuteranopia,
    Protanopia,
    Tritanopia,
    Inverted,
}

/// Magnifier region for screen zoom
#[derive(Debug, Clone, Copy)]
pub struct MagnifierRegion {
    pub center_x: u32,
    pub center_y: u32,
    pub viewport_width: u32,
    pub viewport_height: u32,
}

/// Display-level accessibility features
pub struct DisplayAccessibility {
    pub zoom_level: f32,
    pub high_contrast: bool,
    pub color_filter: ColorFilter,
    pub large_cursor: bool,
    reduce_motion: bool,
    magnifier: MagnifierRegion,
    screen_width: u32,
    screen_height: u32,
    /// Color filter transformation matrix (3x3, row-major, 16.16 fixed-point)
    filter_matrix: [i32; 9],
    /// High contrast boost factor (1.0 = normal, 2.0 = high contrast)
    contrast_boost: f32,
    /// Cursor scale factor when large_cursor is enabled
    cursor_scale: u32,
    /// Sticky keys state
    sticky_keys: bool,
}

/// Build a 3x3 color filter matrix for the given filter type (16.16 fixed-point)
fn build_filter_matrix(filter: ColorFilter) -> [i32; 9] {
    let fp = |v: f32| -> i32 { (v * 65536.0) as i32 };

    match filter {
        ColorFilter::None => [
            fp(1.0),
            fp(0.0),
            fp(0.0),
            fp(0.0),
            fp(1.0),
            fp(0.0),
            fp(0.0),
            fp(0.0),
            fp(1.0),
        ],
        ColorFilter::Grayscale => {
            // ITU-R BT.709 luminance weights
            [
                fp(0.2126),
                fp(0.7152),
                fp(0.0722),
                fp(0.2126),
                fp(0.7152),
                fp(0.0722),
                fp(0.2126),
                fp(0.7152),
                fp(0.0722),
            ]
        }
        ColorFilter::Deuteranopia => {
            // Simulate deuteranopia (green-blind)
            [
                fp(0.625),
                fp(0.375),
                fp(0.0),
                fp(0.700),
                fp(0.300),
                fp(0.0),
                fp(0.0),
                fp(0.300),
                fp(0.700),
            ]
        }
        ColorFilter::Protanopia => {
            // Simulate protanopia (red-blind)
            [
                fp(0.567),
                fp(0.433),
                fp(0.0),
                fp(0.558),
                fp(0.442),
                fp(0.0),
                fp(0.0),
                fp(0.242),
                fp(0.758),
            ]
        }
        ColorFilter::Tritanopia => {
            // Simulate tritanopia (blue-blind)
            [
                fp(0.950),
                fp(0.050),
                fp(0.0),
                fp(0.0),
                fp(0.433),
                fp(0.567),
                fp(0.0),
                fp(0.475),
                fp(0.525),
            ]
        }
        ColorFilter::Inverted => [
            fp(-1.0),
            fp(0.0),
            fp(0.0),
            fp(0.0),
            fp(-1.0),
            fp(0.0),
            fp(0.0),
            fp(0.0),
            fp(-1.0),
        ],
    }
}

/// Apply a 3x3 color filter matrix to an RGB triplet.
/// Input and output are 0..255 per channel.
fn apply_filter_matrix(matrix: &[i32; 9], r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let ri = r as i32;
    let gi = g as i32;
    let bi = b as i32;

    let out_r = (matrix[0] * ri + matrix[1] * gi + matrix[2] * bi) >> 16;
    let out_g = (matrix[3] * ri + matrix[4] * gi + matrix[5] * bi) >> 16;
    let out_b = (matrix[6] * ri + matrix[7] * gi + matrix[8] * bi) >> 16;

    // For inverted filter, add 255 to shift from [-255..0] to [0..255]
    let (out_r, out_g, out_b) = if matrix[0] < 0 && matrix[4] < 0 && matrix[8] < 0 {
        (out_r + 255, out_g + 255, out_b + 255)
    } else {
        (out_r, out_g, out_b)
    };

    let clamp = |v: i32| -> u8 {
        if v < 0 {
            0
        } else if v > 255 {
            255
        } else {
            v as u8
        }
    };
    (clamp(out_r), clamp(out_g), clamp(out_b))
}

impl DisplayAccessibility {
    pub fn new() -> Self {
        crate::serial_println!("[accessibility] display accessibility module created");
        Self {
            zoom_level: 1.0,
            high_contrast: false,
            color_filter: ColorFilter::None,
            large_cursor: false,
            reduce_motion: false,
            magnifier: MagnifierRegion {
                center_x: 960,
                center_y: 540,
                viewport_width: 1920,
                viewport_height: 1080,
            },
            screen_width: 1920,
            screen_height: 1080,
            filter_matrix: build_filter_matrix(ColorFilter::None),
            contrast_boost: 1.0,
            cursor_scale: 1,
            sticky_keys: false,
        }
    }

    pub fn set_zoom(&mut self, level: f32) {
        let clamped = if level < 1.0 {
            1.0
        } else if level > 10.0 {
            10.0
        } else {
            level
        };
        self.zoom_level = clamped;

        // Update magnifier viewport based on zoom level
        let inv_zoom_256 = (256.0 / clamped) as u32;
        self.magnifier.viewport_width = (self.screen_width * inv_zoom_256) / 256;
        self.magnifier.viewport_height = (self.screen_height * inv_zoom_256) / 256;

        // Clamp magnifier center so viewport stays on screen
        self.clamp_magnifier();

        crate::serial_println!(
            "[accessibility] zoom set to {}x, viewport {}x{}",
            clamped as u32,
            self.magnifier.viewport_width,
            self.magnifier.viewport_height
        );
    }

    pub fn set_color_filter(&mut self, filter: ColorFilter) {
        self.color_filter = filter;
        self.filter_matrix = build_filter_matrix(filter);
        crate::serial_println!("[accessibility] color filter set to {:?}", filter);
    }

    /// Enable or disable high contrast mode
    pub fn set_high_contrast(&mut self, enabled: bool) {
        self.high_contrast = enabled;
        self.contrast_boost = if enabled { 2.0 } else { 1.0 };
        crate::serial_println!("[accessibility] high contrast: {}", enabled);
    }

    /// Enable or disable large cursor
    pub fn set_large_cursor(&mut self, enabled: bool) {
        self.large_cursor = enabled;
        self.cursor_scale = if enabled { 3 } else { 1 };
        crate::serial_println!(
            "[accessibility] large cursor: {} (scale={}x)",
            enabled,
            self.cursor_scale
        );
    }

    /// Enable or disable reduced motion
    pub fn set_reduce_motion(&mut self, enabled: bool) {
        self.reduce_motion = enabled;
        crate::serial_println!("[accessibility] reduce motion: {}", enabled);
    }

    /// Move the magnifier center (follows cursor or focus)
    pub fn move_magnifier(&mut self, x: u32, y: u32) {
        self.magnifier.center_x = x;
        self.magnifier.center_y = y;
        self.clamp_magnifier();
    }

    /// Clamp magnifier viewport to screen bounds
    fn clamp_magnifier(&mut self) {
        let half_w = self.magnifier.viewport_width / 2;
        let half_h = self.magnifier.viewport_height / 2;

        if self.magnifier.center_x < half_w {
            self.magnifier.center_x = half_w;
        }
        if self.magnifier.center_y < half_h {
            self.magnifier.center_y = half_h;
        }
        if self.magnifier.center_x + half_w > self.screen_width {
            self.magnifier.center_x = self.screen_width - half_w;
        }
        if self.magnifier.center_y + half_h > self.screen_height {
            self.magnifier.center_y = self.screen_height - half_h;
        }
    }

    /// Apply all active accessibility filters to an RGBA pixel buffer
    pub fn process_buffer(&self, pixels: &mut [u8]) {
        let pixel_count = pixels.len() / 4;
        for i in 0..pixel_count {
            let base = i * 4;
            let mut r = pixels[base];
            let mut g = pixels[base + 1];
            let mut b = pixels[base + 2];

            // Apply color filter
            if !matches!(self.color_filter, ColorFilter::None) {
                let (fr, fg, fb) = apply_filter_matrix(&self.filter_matrix, r, g, b);
                r = fr;
                g = fg;
                b = fb;
            }

            // Apply high contrast boost
            if self.high_contrast {
                let boost = |v: u8| -> u8 {
                    let centered = v as i32 - 128;
                    let boosted = (centered * 2) + 128;
                    if boosted < 0 {
                        0
                    } else if boosted > 255 {
                        255
                    } else {
                        boosted as u8
                    }
                };
                r = boost(r);
                g = boost(g);
                b = boost(b);
            }

            pixels[base] = r;
            pixels[base + 1] = g;
            pixels[base + 2] = b;
        }
    }

    /// Get the source rectangle for the magnifier (what part of the screen to display)
    pub fn magnifier_source_rect(&self) -> (u32, u32, u32, u32) {
        let half_w = self.magnifier.viewport_width / 2;
        let half_h = self.magnifier.viewport_height / 2;
        let x = self.magnifier.center_x.saturating_sub(half_w);
        let y = self.magnifier.center_y.saturating_sub(half_h);
        (
            x,
            y,
            self.magnifier.viewport_width,
            self.magnifier.viewport_height,
        )
    }

    /// Set screen dimensions
    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_width = width;
        self.screen_height = height;
        // Recalculate magnifier
        self.set_zoom(self.zoom_level);
    }

    /// Check if reduced motion is requested
    pub fn should_reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    /// Get the cursor scale factor
    pub fn cursor_scale_factor(&self) -> u32 {
        self.cursor_scale
    }

    /// Enable/disable sticky keys
    pub fn set_sticky_keys(&mut self, enabled: bool) {
        self.sticky_keys = enabled;
        crate::serial_println!("[accessibility] sticky keys: {}", enabled);
    }

    /// Report current accessibility state
    pub fn report(&self) {
        crate::serial_println!(
            "[accessibility] zoom={}x filter={:?} contrast={} large_cursor={} reduce_motion={}",
            self.zoom_level as u32,
            self.color_filter,
            self.high_contrast,
            self.large_cursor,
            self.reduce_motion
        );
    }
}

static ACCESSIBILITY: Mutex<Option<DisplayAccessibility>> = Mutex::new(None);

pub fn init() {
    let a11y = DisplayAccessibility::new();
    let mut acc = ACCESSIBILITY.lock();
    *acc = Some(a11y);
    crate::serial_println!("[accessibility] subsystem initialized");
}

/// Set zoom level from external code
pub fn set_zoom(level: f32) {
    let mut acc = ACCESSIBILITY.lock();
    if let Some(ref mut a) = *acc {
        a.set_zoom(level);
    }
}

/// Set color filter from external code
pub fn set_color_filter(filter: ColorFilter) {
    let mut acc = ACCESSIBILITY.lock();
    if let Some(ref mut a) = *acc {
        a.set_color_filter(filter);
    }
}

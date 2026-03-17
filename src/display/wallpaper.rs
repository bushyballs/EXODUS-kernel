/// Wallpaper engine for Genesis
///
/// Provides: static wallpapers, slideshow cycling, live wallpaper rendering,
/// parallax scrolling, blur-behind-windows, and scaling modes.
///
/// Uses Q16 fixed-point math throughout (no floats).
///
/// Inspired by: Android live wallpapers, macOS dynamic desktops. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

use crate::{serial_print, serial_println};

/// Q16 fixed-point constant: 1.0
const Q16_ONE: i32 = 65536;

/// Q16 multiply
fn q16_mul(a: i32, b: i32) -> i32 {
    ((a as i64 * b as i64) >> 16) as i32
}

/// Q16 from integer
fn q16_from_int(x: i32) -> i32 {
    x << 16
}

/// Wallpaper scaling mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalingMode {
    Stretch, // Stretch to fill, ignoring aspect ratio
    Fill,    // Scale up and crop to fill, preserving aspect ratio
    Fit,     // Scale down to fit inside, with letterboxing
    Center,  // No scaling, center on screen
    Tile,    // Tile the image across the screen
    Span,    // Span across multiple monitors
}

/// Wallpaper type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallpaperType {
    Static,
    Slideshow,
    LiveAnimated,
    LiveParticle,
    SolidColor,
    Gradient,
}

/// A single wallpaper image entry
pub struct WallpaperImage {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>, // ARGB pixel data
}

impl WallpaperImage {
    pub fn new(id: u32, name: &str, width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        WallpaperImage {
            id,
            name: String::from(name),
            width,
            height,
            pixels: alloc::vec![0xFF12121C; size],
        }
    }

    /// Generate a solid color wallpaper
    pub fn solid(id: u32, name: &str, width: u32, height: u32, color: u32) -> Self {
        let size = (width * height) as usize;
        WallpaperImage {
            id,
            name: String::from(name),
            width,
            height,
            pixels: alloc::vec![color; size],
        }
    }

    /// Generate a vertical gradient wallpaper
    pub fn vertical_gradient(
        id: u32,
        name: &str,
        width: u32,
        height: u32,
        top_color: u32,
        bottom_color: u32,
    ) -> Self {
        let size = (width * height) as usize;
        let mut pixels = alloc::vec![0u32; size];
        let top_r = ((top_color >> 16) & 0xFF) as i32;
        let top_g = ((top_color >> 8) & 0xFF) as i32;
        let top_b = (top_color & 0xFF) as i32;
        let bot_r = ((bottom_color >> 16) & 0xFF) as i32;
        let bot_g = ((bottom_color >> 8) & 0xFF) as i32;
        let bot_b = (bottom_color & 0xFF) as i32;

        for y in 0..height {
            let t = if height > 1 {
                ((y as i64 * Q16_ONE as i64) / (height - 1) as i64) as i32
            } else {
                0
            };
            let r = top_r + (q16_mul(bot_r - top_r, t));
            let g = top_g + (q16_mul(bot_g - top_g, t));
            let b = top_b + (q16_mul(bot_b - top_b, t));
            let r = if r < 0 {
                0
            } else if r > 255 {
                255
            } else {
                r
            } as u32;
            let g = if g < 0 {
                0
            } else if g > 255 {
                255
            } else {
                g
            } as u32;
            let b = if b < 0 {
                0
            } else if b > 255 {
                255
            } else {
                b
            } as u32;
            let pixel = 0xFF000000 | (r << 16) | (g << 8) | b;
            for x in 0..width {
                pixels[(y * width + x) as usize] = pixel;
            }
        }
        WallpaperImage {
            id,
            name: String::from(name),
            width,
            height,
            pixels,
        }
    }

    /// Get a pixel with bounds checking
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) as usize]
        } else {
            0xFF000000
        }
    }
}

/// Slideshow configuration
pub struct SlideshowConfig {
    pub interval_ms: u64,
    pub transition_ms: u64,
    pub shuffle: bool,
    pub image_ids: Vec<u32>,
    pub current_index: usize,
    pub last_switch: u64,
    pub transitioning: bool,
    pub transition_progress: i32, // Q16: 0..Q16_ONE
}

impl SlideshowConfig {
    pub fn new(interval_ms: u64) -> Self {
        SlideshowConfig {
            interval_ms,
            transition_ms: 1000,
            shuffle: false,
            image_ids: Vec::new(),
            current_index: 0,
            last_switch: 0,
            transitioning: false,
            transition_progress: 0,
        }
    }

    /// Get the current image id
    pub fn current_id(&self) -> Option<u32> {
        self.image_ids.get(self.current_index).copied()
    }

    /// Get the next image id (for transitions)
    pub fn next_id(&self) -> Option<u32> {
        if self.image_ids.is_empty() {
            return None;
        }
        let next = (self.current_index + 1) % self.image_ids.len();
        self.image_ids.get(next).copied()
    }

    /// Advance to next image
    pub fn advance(&mut self) {
        if self.image_ids.is_empty() {
            return;
        }
        self.current_index = (self.current_index + 1) % self.image_ids.len();
    }
}

/// Parallax layer for scrolling wallpaper
pub struct ParallaxLayer {
    pub image_id: u32,
    pub scroll_speed: i32, // Q16: multiplier (Q16_ONE = 1x speed)
    pub x_offset: i32,     // Q16: current horizontal offset
    pub y_offset: i32,     // Q16: current vertical offset
    pub opacity: i32,      // Q16: layer opacity
}

impl ParallaxLayer {
    pub fn new(image_id: u32, speed: i32) -> Self {
        ParallaxLayer {
            image_id,
            scroll_speed: speed,
            x_offset: 0,
            y_offset: 0,
            opacity: Q16_ONE,
        }
    }

    /// Update the layer offset based on cursor or scroll position
    pub fn update_offset(&mut self, cursor_x_q16: i32, cursor_y_q16: i32) {
        self.x_offset = q16_mul(cursor_x_q16, self.scroll_speed);
        self.y_offset = q16_mul(cursor_y_q16, self.scroll_speed);
    }
}

/// Blur kernel size for blur-behind-windows
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlurLevel {
    None,
    Light,  // 3x3 box blur
    Medium, // 5x5 box blur
    Heavy,  // 7x7 box blur
}

impl BlurLevel {
    pub fn kernel_size(&self) -> u32 {
        match self {
            BlurLevel::None => 0,
            BlurLevel::Light => 3,
            BlurLevel::Medium => 5,
            BlurLevel::Heavy => 7,
        }
    }
}

/// Apply a simple box blur to a rectangular region of a pixel buffer
pub fn box_blur_region(
    pixels: &mut Vec<u32>,
    buf_width: u32,
    rx: u32,
    ry: u32,
    rw: u32,
    rh: u32,
    kernel: u32,
) {
    if kernel < 2 || rw == 0 || rh == 0 {
        return;
    }
    let half = (kernel / 2) as i32;
    let _area = kernel * kernel;

    // Work on a copy of the region to avoid read-write aliasing
    let region_size = (rw * rh) as usize;
    let mut temp = alloc::vec![0u32; region_size];
    for dy in 0..rh {
        for dx in 0..rw {
            temp[(dy * rw + dx) as usize] = pixels[((ry + dy) * buf_width + rx + dx) as usize];
        }
    }

    for dy in 0..rh {
        for dx in 0..rw {
            let mut sum_r: u32 = 0;
            let mut sum_g: u32 = 0;
            let mut sum_b: u32 = 0;
            let mut count: u32 = 0;
            for ky in -half..=half {
                for kx in -half..=half {
                    let sx = dx as i32 + kx;
                    let sy = dy as i32 + ky;
                    if sx >= 0 && sy >= 0 && sx < rw as i32 && sy < rh as i32 {
                        let p = temp[(sy as u32 * rw + sx as u32) as usize];
                        sum_r += (p >> 16) & 0xFF;
                        sum_g += (p >> 8) & 0xFF;
                        sum_b += p & 0xFF;
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let r = sum_r / count;
                let g = sum_g / count;
                let b = sum_b / count;
                let idx = ((ry + dy) * buf_width + rx + dx) as usize;
                pixels[idx] = 0xFF000000 | (r << 16) | (g << 8) | b;
            }
        }
    }
}

/// Live wallpaper particle (for animated backgrounds)
pub struct Particle {
    pub x: i32,    // Q16 position
    pub y: i32,    // Q16 position
    pub vx: i32,   // Q16 velocity
    pub vy: i32,   // Q16 velocity
    pub life: i32, // Q16: remaining life (Q16_ONE = full, 0 = dead)
    pub size: u32,
    pub color: u32,
}

/// The wallpaper engine
pub struct WallpaperEngine {
    pub wallpaper_type: WallpaperType,
    pub scaling: ScalingMode,
    pub images: Vec<WallpaperImage>,
    pub active_image_id: u32,
    pub slideshow: SlideshowConfig,
    pub parallax_layers: Vec<ParallaxLayer>,
    pub blur_level: BlurLevel,
    pub particles: Vec<Particle>,
    pub max_particles: usize,
    pub tint_color: u32,   // ARGB tint overlay
    pub tint_opacity: i32, // Q16
    pub next_image_id: u32,
    pub screen_width: u32,
    pub screen_height: u32,
}

impl WallpaperEngine {
    const fn new() -> Self {
        WallpaperEngine {
            wallpaper_type: WallpaperType::SolidColor,
            scaling: ScalingMode::Fill,
            images: Vec::new(),
            active_image_id: 0,
            slideshow: SlideshowConfig {
                interval_ms: 30000,
                transition_ms: 1000,
                shuffle: false,
                image_ids: Vec::new(),
                current_index: 0,
                last_switch: 0,
                transitioning: false,
                transition_progress: 0,
            },
            parallax_layers: Vec::new(),
            blur_level: BlurLevel::None,
            particles: Vec::new(),
            max_particles: 64,
            tint_color: 0x00000000,
            tint_opacity: 0,
            next_image_id: 1,
            screen_width: 1024,
            screen_height: 768,
        }
    }

    /// Register a wallpaper image
    pub fn add_image(&mut self, mut img: WallpaperImage) -> u32 {
        let id = self.next_image_id;
        self.next_image_id = self.next_image_id.saturating_add(1);
        img.id = id;
        self.images.push(img);
        id
    }

    /// Set the active wallpaper by id
    pub fn set_active(&mut self, image_id: u32) {
        self.active_image_id = image_id;
        self.wallpaper_type = WallpaperType::Static;
    }

    /// Set solid color wallpaper
    pub fn set_solid_color(&mut self, color: u32) {
        self.wallpaper_type = WallpaperType::SolidColor;
        self.tint_color = color;
    }

    /// Configure slideshow
    pub fn start_slideshow(&mut self, image_ids: Vec<u32>, interval_ms: u64, now_ms: u64) {
        self.wallpaper_type = WallpaperType::Slideshow;
        self.slideshow.image_ids = image_ids;
        self.slideshow.interval_ms = interval_ms;
        self.slideshow.current_index = 0;
        self.slideshow.last_switch = now_ms;
        self.slideshow.transitioning = false;
    }

    /// Update slideshow state
    pub fn update_slideshow(&mut self, now_ms: u64) {
        if self.wallpaper_type != WallpaperType::Slideshow {
            return;
        }
        if self.slideshow.image_ids.is_empty() {
            return;
        }

        let since_switch = now_ms.saturating_sub(self.slideshow.last_switch);

        if self.slideshow.transitioning {
            let t = ((since_switch.saturating_sub(self.slideshow.interval_ms) as i64
                * Q16_ONE as i64)
                / self.slideshow.transition_ms as i64) as i32;
            self.slideshow.transition_progress = if t > Q16_ONE { Q16_ONE } else { t };

            if self.slideshow.transition_progress >= Q16_ONE {
                self.slideshow.advance();
                if let Some(id) = self.slideshow.current_id() {
                    self.active_image_id = id;
                }
                self.slideshow.transitioning = false;
                self.slideshow.last_switch = now_ms;
                self.slideshow.transition_progress = 0;
            }
        } else if since_switch >= self.slideshow.interval_ms {
            self.slideshow.transitioning = true;
        }
    }

    /// Spawn a particle for live wallpaper
    pub fn spawn_particle(&mut self, x: i32, y: i32, vx: i32, vy: i32, color: u32) {
        if self.particles.len() >= self.max_particles {
            // Remove oldest dead or first particle
            if !self.particles.is_empty() {
                self.particles.remove(0);
            }
        }
        self.particles.push(Particle {
            x,
            y,
            vx,
            vy,
            life: Q16_ONE,
            size: 2,
            color,
        });
    }

    /// Update particles
    pub fn update_particles(&mut self) {
        let decay = Q16_ONE / 120; // die over ~2 seconds at 60fps
        for p in &mut self.particles {
            p.x += p.vx;
            p.y += p.vy;
            p.life -= decay;
        }
        self.particles.retain(|p| p.life > 0);
    }

    /// Get the scaled destination rect for an image on screen
    /// Returns (dst_x, dst_y, dst_w, dst_h) for the given scaling mode
    pub fn compute_scaling(&self, img_w: u32, img_h: u32) -> (i32, i32, u32, u32) {
        let sw = self.screen_width;
        let sh = self.screen_height;
        match self.scaling {
            ScalingMode::Stretch => (0, 0, sw, sh),
            ScalingMode::Center => {
                let dx = (sw as i32 - img_w as i32) / 2;
                let dy = (sh as i32 - img_h as i32) / 2;
                (dx, dy, img_w, img_h)
            }
            ScalingMode::Fill => {
                // Scale up to cover the screen, crop excess
                let scale_x = q16_from_int(sw as i32) / (img_w as i32).max(1);
                let scale_y = q16_from_int(sh as i32) / (img_h as i32).max(1);
                let scale = if scale_x > scale_y { scale_x } else { scale_y };
                let dw = (q16_mul(q16_from_int(img_w as i32), scale) >> 16) as u32;
                let dh = (q16_mul(q16_from_int(img_h as i32), scale) >> 16) as u32;
                let dx = (sw as i32 - dw as i32) / 2;
                let dy = (sh as i32 - dh as i32) / 2;
                (dx, dy, dw, dh)
            }
            ScalingMode::Fit => {
                // Scale down to fit inside the screen
                let scale_x = q16_from_int(sw as i32) / (img_w as i32).max(1);
                let scale_y = q16_from_int(sh as i32) / (img_h as i32).max(1);
                let scale = if scale_x < scale_y { scale_x } else { scale_y };
                let dw = (q16_mul(q16_from_int(img_w as i32), scale) >> 16) as u32;
                let dh = (q16_mul(q16_from_int(img_h as i32), scale) >> 16) as u32;
                let dx = (sw as i32 - dw as i32) / 2;
                let dy = (sh as i32 - dh as i32) / 2;
                (dx, dy, dw, dh)
            }
            ScalingMode::Tile | ScalingMode::Span => (0, 0, sw, sh),
        }
    }
}

static WALLPAPER: Mutex<WallpaperEngine> = Mutex::new(WallpaperEngine::new());

/// Initialize the wallpaper engine
pub fn init() {
    serial_println!(
        "    [wallpaper] Wallpaper engine initialized (static, slideshow, live, parallax, blur)"
    );
}

/// Set a solid color wallpaper
pub fn set_solid(color: u32) {
    WALLPAPER.lock().set_solid_color(color);
}

/// Add an image to the wallpaper library and return its id
pub fn add_image(img: WallpaperImage) -> u32 {
    WALLPAPER.lock().add_image(img)
}

/// Set the active wallpaper
pub fn set_active(image_id: u32) {
    WALLPAPER.lock().set_active(image_id);
}

/// Set scaling mode
pub fn set_scaling(mode: ScalingMode) {
    WALLPAPER.lock().scaling = mode;
}

/// Set blur level for behind-window regions
pub fn set_blur(level: BlurLevel) {
    WALLPAPER.lock().blur_level = level;
}

/// Update wallpaper state (call each frame)
pub fn update(now_ms: u64) {
    let mut wp = WALLPAPER.lock();
    wp.update_slideshow(now_ms);
    wp.update_particles();
}

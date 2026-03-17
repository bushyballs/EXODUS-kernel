use crate::serial_println;
/// AR camera feed overlay for Genesis
///
/// Composites AR content (3D objects, text labels, 2D HUD elements) onto a
/// live camera feed.  The overlay pipeline consists of three layers:
///
///   1. **Camera layer** — raw RGBA frames from the camera driver.
///   2. **Depth layer** — optional depth map used for occlusion masking.
///   3. **Overlay layer** — pre-rendered AR content (transparent pixels are
///      skipped during compositing).
///
/// ## Frame format
///
/// All buffers are `width × height × 4 bytes` RGBA8888.
/// Alpha = 0 means fully transparent; alpha = 255 means fully opaque.
///
/// ## Coordinate system
///
/// Normalised Device Coordinates (NDC): (-1,-1) bottom-left, (+1,+1) top-right.
/// Screen-space conversions use the active `OverlayConfig` resolution.
///
/// ## `no_std` design
///
/// Frame buffers are thin wrappers around slices provided by the caller.
/// No heap allocation is needed for the compositing operation itself; the
/// static `OverlayState` holds only configuration.
///
/// All code is original — Hoags Inc. (c) 2026.

#[allow(dead_code)]
use crate::sync::Mutex;

// ============================================================================
// Configuration
// ============================================================================

/// Maximum number of AR overlay primitives queued per frame
const MAX_OVERLAY_PRIMITIVES: usize = 256;

/// Active overlay configuration
#[derive(Clone, Copy, Debug)]
pub struct OverlayConfig {
    /// Camera frame width in pixels
    pub frame_width: u32,
    /// Camera frame height in pixels
    pub frame_height: u32,
    /// Whether depth-based occlusion is enabled
    pub occlusion_enabled: bool,
    /// Overall AR opacity (0-255; 255 = fully opaque AR content)
    pub ar_opacity: u8,
    /// Whether to draw a HUD frame counter for debugging
    pub debug_hud: bool,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        OverlayConfig {
            frame_width: 1920,
            frame_height: 1080,
            occlusion_enabled: true,
            ar_opacity: 255,
            debug_hud: false,
        }
    }
}

// ============================================================================
// Overlay primitive types
// ============================================================================

/// A single RGBA pixel colour
#[derive(Clone, Copy, Debug, Default)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba { r, g, b, a }
    }
    pub const TRANSPARENT: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const WHITE: Rgba = Rgba {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const BLACK: Rgba = Rgba {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const AMBER: Rgba = Rgba {
        r: 245,
        g: 158,
        b: 11,
        a: 255,
    };
}

/// NDC 2D point (-1.0 to +1.0, stored as i32 × 1_000_000 for no_std)
#[derive(Clone, Copy, Debug, Default)]
pub struct NdcPoint {
    /// x in range [-1_000_000, +1_000_000] representing [-1.0, +1.0]
    pub x_fixed: i32,
    /// y in range [-1_000_000, +1_000_000] representing [-1.0, +1.0]
    pub y_fixed: i32,
}

impl NdcPoint {
    /// Create from integer screen coordinates given frame dimensions.
    pub fn from_screen(sx: i32, sy: i32, width: u32, height: u32) -> Self {
        let w = width as i32;
        let h = height as i32;
        NdcPoint {
            x_fixed: (sx * 2_000_000 / w) - 1_000_000,
            y_fixed: 1_000_000 - (sy * 2_000_000 / h),
        }
    }

    /// Convert to screen (pixel) coordinates.
    pub fn to_screen(&self, width: u32, height: u32) -> (i32, i32) {
        let w = width as i32;
        let h = height as i32;
        let sx = (self.x_fixed + 1_000_000) * w / 2_000_000;
        let sy = h - (self.y_fixed + 1_000_000) * h / 2_000_000;
        (sx, sy)
    }
}

/// Overlay primitive variant
#[derive(Clone, Copy, Debug)]
pub enum OverlayPrimitive {
    /// Filled rectangle in screen coordinates
    FillRect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: Rgba,
    },
    /// Outline rectangle
    DrawRect {
        x: i32,
        y: i32,
        w: u32,
        h: u32,
        color: Rgba,
    },
    /// Text label at screen position
    Text {
        x: i32,
        y: i32,
        text: [u8; 64],
        text_len: usize,
        color: Rgba,
        scale: u8, // 1 = normal, 2 = double, etc.
    },
    /// 2D circle at screen coordinates
    Circle {
        cx: i32,
        cy: i32,
        radius: u32,
        color: Rgba,
        filled: bool,
    },
    /// Draw a line between two screen points
    Line {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        color: Rgba,
    },
    /// Crosshair (debug / tracking marker)
    Crosshair {
        cx: i32,
        cy: i32,
        size: u32,
        color: Rgba,
    },
}

// ============================================================================
// Overlay state
// ============================================================================

struct OverlayState {
    config: OverlayConfig,
    primitives: [Option<OverlayPrimitive>; MAX_OVERLAY_PRIMITIVES],
    prim_count: usize,
    frame_counter: u64,
    active: bool,
}

impl OverlayState {
    const fn new() -> Self {
        OverlayState {
            config: OverlayConfig {
                frame_width: 1920,
                frame_height: 1080,
                occlusion_enabled: true,
                ar_opacity: 255,
                debug_hud: false,
            },
            primitives: [const { None }; MAX_OVERLAY_PRIMITIVES],
            prim_count: 0,
            frame_counter: 0,
            active: false,
        }
    }

    fn clear_primitives(&mut self) {
        for p in self.primitives.iter_mut() {
            *p = None;
        }
        self.prim_count = 0;
    }

    fn push(&mut self, prim: OverlayPrimitive) -> bool {
        if self.prim_count >= MAX_OVERLAY_PRIMITIVES {
            return false;
        }
        self.primitives[self.prim_count] = Some(prim);
        self.prim_count += 1;
        true
    }
}

static OVERLAY: Mutex<OverlayState> = Mutex::new(OverlayState::new());

// ============================================================================
// Public API
// ============================================================================

/// Initialise the camera overlay subsystem.
pub fn init() {
    let mut s = OVERLAY.lock();
    s.active = true;
    serial_println!(
        "    AR/camera_overlay: compositing pipeline ready ({}×{})",
        s.config.frame_width,
        s.config.frame_height
    );
}

/// Update the overlay configuration.
pub fn configure(cfg: OverlayConfig) {
    OVERLAY.lock().config = cfg;
}

/// Get a copy of the current config.
pub fn get_config() -> OverlayConfig {
    OVERLAY.lock().config
}

/// Begin a new frame: clear all queued primitives.
pub fn begin_frame() {
    let mut s = OVERLAY.lock();
    s.clear_primitives();
    s.frame_counter = s.frame_counter.saturating_add(1);
}

/// Queue a filled rectangle overlay.
pub fn add_fill_rect(x: i32, y: i32, w: u32, h: u32, color: Rgba) -> bool {
    OVERLAY
        .lock()
        .push(OverlayPrimitive::FillRect { x, y, w, h, color })
}

/// Queue a rectangle outline overlay.
pub fn add_draw_rect(x: i32, y: i32, w: u32, h: u32, color: Rgba) -> bool {
    OVERLAY
        .lock()
        .push(OverlayPrimitive::DrawRect { x, y, w, h, color })
}

/// Queue a text label overlay.
pub fn add_text(x: i32, y: i32, text: &[u8], color: Rgba, scale: u8) -> bool {
    let mut t = [0u8; 64];
    let tlen = text.len().min(63);
    t[..tlen].copy_from_slice(&text[..tlen]);
    OVERLAY.lock().push(OverlayPrimitive::Text {
        x,
        y,
        text: t,
        text_len: tlen,
        color,
        scale,
    })
}

/// Queue a circle overlay.
pub fn add_circle(cx: i32, cy: i32, radius: u32, color: Rgba, filled: bool) -> bool {
    OVERLAY.lock().push(OverlayPrimitive::Circle {
        cx,
        cy,
        radius,
        color,
        filled,
    })
}

/// Queue a line overlay.
pub fn add_line(x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba) -> bool {
    OVERLAY.lock().push(OverlayPrimitive::Line {
        x0,
        y0,
        x1,
        y1,
        color,
    })
}

/// Queue a crosshair overlay (useful for tracking markers).
pub fn add_crosshair(cx: i32, cy: i32, size: u32, color: Rgba) -> bool {
    OVERLAY.lock().push(OverlayPrimitive::Crosshair {
        cx,
        cy,
        size,
        color,
    })
}

/// Composite the overlay onto a camera frame buffer.
///
/// `camera_frame` — mutable RGBA8888 slice of size `width * height * 4`.
/// The overlay primitives rasterised during this frame are composited using
/// standard alpha blending:
///   out.rgb = src.rgb * src.a/255 + dst.rgb * (1 - src.a/255)
///
/// Returns the number of primitives composited.
pub fn composite(camera_frame: &mut [u8]) -> usize {
    let state = OVERLAY.lock();
    if !state.active {
        return 0;
    }

    let width = state.config.frame_width;
    let height = state.config.frame_height;
    let ar_opacity = state.config.ar_opacity;
    let n = state.prim_count;

    for prim_opt in state.primitives[..n].iter() {
        match prim_opt {
            Some(OverlayPrimitive::FillRect { x, y, w, h, color }) => {
                composite_fill_rect(
                    camera_frame,
                    width,
                    height,
                    *x,
                    *y,
                    *w,
                    *h,
                    blend_rgba(*color, ar_opacity),
                );
            }
            Some(OverlayPrimitive::Circle {
                cx,
                cy,
                radius,
                color,
                filled,
            }) => {
                composite_circle(
                    camera_frame,
                    width,
                    height,
                    *cx,
                    *cy,
                    *radius,
                    blend_rgba(*color, ar_opacity),
                    *filled,
                );
            }
            Some(OverlayPrimitive::Crosshair {
                cx,
                cy,
                size,
                color,
            }) => {
                let c = blend_rgba(*color, ar_opacity);
                composite_hline(
                    camera_frame,
                    width,
                    height,
                    *cx - *size as i32,
                    *cx + *size as i32,
                    *cy,
                    c,
                );
                composite_vline(
                    camera_frame,
                    width,
                    height,
                    *cx,
                    *cy - *size as i32,
                    *cy + *size as i32,
                    c,
                );
            }
            Some(OverlayPrimitive::Line {
                x0,
                y0,
                x1,
                y1,
                color,
            }) => {
                composite_line(
                    camera_frame,
                    width,
                    height,
                    *x0,
                    *y0,
                    *x1,
                    *y1,
                    blend_rgba(*color, ar_opacity),
                );
            }
            // Text and DrawRect require a font; emit a placeholder bounding box
            Some(OverlayPrimitive::DrawRect { x, y, w, h, color }) => {
                composite_rect_outline(
                    camera_frame,
                    width,
                    height,
                    *x,
                    *y,
                    *w,
                    *h,
                    blend_rgba(*color, ar_opacity),
                );
            }
            Some(OverlayPrimitive::Text { x, y, color, .. }) => {
                // Minimal placeholder: draw a 4×4 marker at text origin
                composite_fill_rect(
                    camera_frame,
                    width,
                    height,
                    *x,
                    *y,
                    4,
                    4,
                    blend_rgba(*color, ar_opacity),
                );
            }
            None => {}
        }
    }
    n
}

/// Scale an Rgba colour's alpha by `global_opacity` (0-255).
#[inline]
fn blend_rgba(mut color: Rgba, global_opacity: u8) -> Rgba {
    color.a = (color.a as u16 * global_opacity as u16 / 255) as u8;
    color
}

/// Write one RGBA pixel with alpha-blending over the camera frame.
#[inline]
fn composite_pixel(frame: &mut [u8], width: u32, height: u32, x: i32, y: i32, src: Rgba) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let idx = ((y as usize) * width as usize + x as usize) * 4;
    if idx + 3 >= frame.len() {
        return;
    }
    let a = src.a as u32;
    let ia = 255 - a;
    frame[idx] = ((src.r as u32 * a + frame[idx] as u32 * ia) / 255) as u8;
    frame[idx + 1] = ((src.g as u32 * a + frame[idx + 1] as u32 * ia) / 255) as u8;
    frame[idx + 2] = ((src.b as u32 * a + frame[idx + 2] as u32 * ia) / 255) as u8;
    frame[idx + 3] = 255; // output is fully opaque
}

fn composite_fill_rect(
    frame: &mut [u8],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rw: u32,
    rh: u32,
    color: Rgba,
) {
    for row in y..(y + rh as i32) {
        for col in x..(x + rw as i32) {
            composite_pixel(frame, w, h, col, row, color);
        }
    }
}

fn composite_rect_outline(
    frame: &mut [u8],
    w: u32,
    h: u32,
    x: i32,
    y: i32,
    rw: u32,
    rh: u32,
    color: Rgba,
) {
    composite_hline(frame, w, h, x, x + rw as i32 - 1, y, color);
    composite_hline(frame, w, h, x, x + rw as i32 - 1, y + rh as i32 - 1, color);
    composite_vline(frame, w, h, x, y, y + rh as i32 - 1, color);
    composite_vline(frame, w, h, x + rw as i32 - 1, y, y + rh as i32 - 1, color);
}

fn composite_hline(frame: &mut [u8], w: u32, h: u32, x0: i32, x1: i32, y: i32, color: Rgba) {
    for x in x0..=x1 {
        composite_pixel(frame, w, h, x, y, color);
    }
}

fn composite_vline(frame: &mut [u8], w: u32, h: u32, x: i32, y0: i32, y1: i32, color: Rgba) {
    for y in y0..=y1 {
        composite_pixel(frame, w, h, x, y, color);
    }
}

fn composite_circle(
    frame: &mut [u8],
    w: u32,
    h: u32,
    cx: i32,
    cy: i32,
    r: u32,
    color: Rgba,
    filled: bool,
) {
    if r == 0 {
        composite_pixel(frame, w, h, cx, cy, color);
        return;
    }
    let r = r as i32;
    let mut x = 0i32;
    let mut y = r;
    let mut d = 1 - r;

    while x <= y {
        if filled {
            composite_hline(frame, w, h, cx - x, cx + x, cy + y, color);
            composite_hline(frame, w, h, cx - x, cx + x, cy - y, color);
            composite_hline(frame, w, h, cx - y, cx + y, cy + x, color);
            composite_hline(frame, w, h, cx - y, cx + y, cy - x, color);
        } else {
            for (px, py) in [
                (cx + x, cy + y),
                (cx - x, cy + y),
                (cx + x, cy - y),
                (cx - x, cy - y),
                (cx + y, cy + x),
                (cx - y, cy + x),
                (cx + y, cy - x),
                (cx - y, cy - x),
            ] {
                composite_pixel(frame, w, h, px, py, color);
            }
        }
        if d < 0 {
            d += 2 * x + 3;
        } else {
            d += 2 * (x - y) + 5;
            y -= 1;
        }
        x += 1;
    }
}

/// Bresenham line rasteriser
fn composite_line(
    frame: &mut [u8],
    fw: u32,
    fh: u32,
    mut x0: i32,
    mut y0: i32,
    x1: i32,
    y1: i32,
    color: Rgba,
) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        composite_pixel(frame, fw, fh, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

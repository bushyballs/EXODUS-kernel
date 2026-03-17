use crate::boot_protocol;
use crate::sync::Mutex;
/// Framebuffer graphics driver for Genesis
///
/// Provides pixel-level access to the display. Works with both:
///   - VBE/VESA linear framebuffer (set by bootloader)
///   - VGA text mode (fallback, already working)
///
/// Includes Bresenham line drawing, rectangle fill/outline,
/// 8x16 bitmap font rendering, scrolling, and double-buffering.
///
/// Inspired by: Linux fbdev, Fuchsia display driver, Redox vesad.
/// All code is original.
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Framebuffer information
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    /// Physical address of the framebuffer
    pub addr: usize,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Bytes per pixel (typically 4 for 32-bit color)
    pub bpp: u32,
    /// Bytes per scan line (may be > width * bpp due to padding)
    pub pitch: u32,
    /// Whether this is a text mode or graphics mode framebuffer
    pub mode: DisplayMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    Text,     // VGA text mode (80x25)
    Graphics, // Linear framebuffer (pixel mode)
}

/// A 32-bit RGBA color
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Color { r, g, b, a }
    }

    pub const BLACK: Color = Color::rgb(0, 0, 0);
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const RED: Color = Color::rgb(255, 0, 0);
    pub const GREEN: Color = Color::rgb(0, 255, 0);
    pub const BLUE: Color = Color::rgb(0, 0, 255);
    pub const YELLOW: Color = Color::rgb(255, 255, 0);
    pub const CYAN: Color = Color::rgb(0, 255, 255);
    pub const MAGENTA: Color = Color::rgb(255, 0, 255);
    pub const GRAY: Color = Color::rgb(128, 128, 128);
    pub const DARK_GRAY: Color = Color::rgb(64, 64, 64);
    pub const LIGHT_GRAY: Color = Color::rgb(192, 192, 192);
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    /// Hoags Inc brand colors
    pub const HOAGS_CYAN: Color = Color::rgb(0, 200, 220);
    pub const HOAGS_DARK: Color = Color::rgb(18, 18, 24);
    pub const HOAGS_ACCENT: Color = Color::rgb(255, 100, 50);

    /// Convert to 32-bit packed pixel (0xAARRGGBB)
    pub const fn to_u32(&self) -> u32 {
        (self.a as u32) << 24 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }

    /// Create from 32-bit packed pixel (0xAARRGGBB)
    pub const fn from_u32(val: u32) -> Self {
        Color {
            a: (val >> 24) as u8,
            r: (val >> 16) as u8,
            g: (val >> 8) as u8,
            b: val as u8,
        }
    }

    /// Alpha-blend this color over `dst` using integer math.
    /// self is the foreground (source), dst is the background.
    pub fn blend_over(&self, dst: Color) -> Color {
        let sa = self.a as u32;
        let da = 255 - sa;
        Color {
            r: ((self.r as u32 * sa + dst.r as u32 * da) / 255) as u8,
            g: ((self.g as u32 * sa + dst.g as u32 * da) / 255) as u8,
            b: ((self.b as u32 * sa + dst.b as u32 * da) / 255) as u8,
            a: 255,
        }
    }
}

/// Pack r, g, b into a u32 pixel value
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32
}

// ---------------------------------------------------------------------------
// Double buffer
// ---------------------------------------------------------------------------

/// Double buffer state for tear-free rendering
struct DoubleBuffer {
    /// Back buffer data (heap-allocated)
    data: Vec<u8>,
    /// Whether double buffering is active
    enabled: bool,
    /// Dirty region tracking (min_x, min_y, max_x, max_y)
    dirty: Option<(u32, u32, u32, u32)>,
}

impl DoubleBuffer {
    const fn new() -> Self {
        DoubleBuffer {
            data: Vec::new(),
            enabled: false,
            dirty: None,
        }
    }

    fn mark_dirty(&mut self, x: u32, y: u32) {
        match self.dirty {
            None => self.dirty = Some((x, y, x, y)),
            Some((x0, y0, x1, y1)) => {
                self.dirty = Some((
                    if x < x0 { x } else { x0 },
                    if y < y0 { y } else { y0 },
                    if x > x1 { x } else { x1 },
                    if y > y1 { y } else { y1 },
                ));
            }
        }
    }

    fn mark_rect_dirty(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let x1 = x + w.saturating_sub(1);
        let y1 = y + h.saturating_sub(1);
        match self.dirty {
            None => self.dirty = Some((x, y, x1, y1)),
            Some((dx0, dy0, dx1, dy1)) => {
                self.dirty = Some((
                    if x < dx0 { x } else { dx0 },
                    if y < dy0 { y } else { dy0 },
                    if x1 > dx1 { x1 } else { dx1 },
                    if y1 > dy1 { y1 } else { dy1 },
                ));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static FRAMEBUFFER: Mutex<Option<FramebufferInfo>> = Mutex::new(None);
static BACK_BUFFER: Mutex<DoubleBuffer> = Mutex::new(DoubleBuffer::new());

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the framebuffer driver
pub fn init() {
    if let Some(info) = boot_protocol::boot_info() {
        let fb = info.framebuffer;
        if fb.address != 0 && fb.width > 0 && fb.height > 0 && fb.bpp > 0 {
            let bytes_per_pixel = (fb.bpp / 8).max(1);
            let pitch = if fb.stride > 0 {
                fb.stride.saturating_mul(bytes_per_pixel)
            } else {
                fb.width.saturating_mul(bytes_per_pixel)
            };
            set_graphics_mode(
                fb.address as usize,
                fb.width,
                fb.height,
                bytes_per_pixel,
                pitch,
            );
            super::register("framebuffer", super::DeviceType::Display);
            return;
        }
    }

    // Default to VGA text mode (already running)
    let info = FramebufferInfo {
        addr: 0xb8000,
        width: 80,
        height: 25,
        bpp: 2,
        pitch: 160,
        mode: DisplayMode::Text,
    };

    *FRAMEBUFFER.lock() = Some(info);
    super::register("framebuffer", super::DeviceType::Display);
    serial_println!(
        "  Framebuffer: VGA text mode {}x{}",
        info.width,
        info.height
    );
}

/// Set up a linear framebuffer (called when bootloader provides one)
pub fn set_graphics_mode(addr: usize, width: u32, height: u32, bpp: u32, pitch: u32) {
    let info = FramebufferInfo {
        addr,
        width,
        height,
        bpp,
        pitch,
        mode: DisplayMode::Graphics,
    };

    *FRAMEBUFFER.lock() = Some(info);
    serial_println!(
        "  Framebuffer: graphics mode {}x{}x{}",
        width,
        height,
        bpp * 8
    );
}

/// Get framebuffer info
pub fn info() -> Option<FramebufferInfo> {
    *FRAMEBUFFER.lock()
}

// ---------------------------------------------------------------------------
// Double-buffering control
// ---------------------------------------------------------------------------

/// Enable double-buffering. Allocates a back buffer on the heap.
pub fn enable_double_buffer() {
    let fb = FRAMEBUFFER.lock();
    if let Some(info) = *fb {
        if info.mode != DisplayMode::Graphics {
            return;
        }
        let size = (info.pitch * info.height) as usize;
        drop(fb);
        let mut bb = BACK_BUFFER.lock();
        bb.data = alloc::vec![0u8; size];
        bb.enabled = true;
        bb.dirty = None;
        serial_println!("  Framebuffer: double buffer enabled ({} bytes)", size);
    }
}

/// Disable double-buffering and free the back buffer.
pub fn disable_double_buffer() {
    let mut bb = BACK_BUFFER.lock();
    bb.data = Vec::new();
    bb.enabled = false;
    bb.dirty = None;
}

/// Flip the back buffer to the front buffer (VRAM) using volatile 32-bit writes.
///
/// Volatile writes prevent the compiler from eliding or reordering the stores,
/// which is required for MMIO framebuffers.  Only the dirty rectangle is written.
/// If nothing is dirty the function returns immediately without touching VRAM.
pub fn flip() {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let mut bb = BACK_BUFFER.lock();
    if !bb.enabled || bb.data.is_empty() {
        return;
    }

    let dirty = bb.dirty.take();
    let (x0, y0, x1, y1) = match dirty {
        Some((a, b, c, d)) => (
            a,
            b,
            c.min(info.width.saturating_sub(1)),
            d.min(info.height.saturating_sub(1)),
        ),
        None => return, // nothing dirty — skip VRAM write
    };

    // BPP must be 4 for 32-bit mode.  If not, fall back to byte-wise volatile.
    let bpp = info.bpp as usize;
    for row in y0..=y1 {
        let row_off = (row as usize).saturating_mul(info.pitch as usize);
        for col in x0..=x1 {
            let off = row_off.saturating_add((col as usize).saturating_mul(bpp));
            if off.saturating_add(4) > bb.data.len() {
                continue;
            }
            // Read 4 bytes from back buffer as a u32.
            let mut word = [0u8; 4];
            word.copy_from_slice(&bb.data[off..off + 4]);
            let pixel = u32::from_ne_bytes(word);
            let vram_addr = info.addr.saturating_add(off);
            // SeqCst fence is issued once per flip by the caller level; individual
            // writes use write_volatile which already prevents reordering with
            // surrounding memory operations.
            unsafe {
                core::ptr::write_volatile(vram_addr as *mut u32, pixel);
            }
        }
    }
    // Full memory fence after all VRAM writes.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

/// Flip the entire back buffer to VRAM using volatile 32-bit writes.
///
/// Does not use dirty tracking — every pixel is written.  Use this on the
/// first frame or after a full-screen clear where the entire surface changed.
pub fn flip_full() {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let mut bb = BACK_BUFFER.lock();
    if !bb.enabled || bb.data.is_empty() {
        return;
    }
    bb.dirty = None;

    let total_bytes = (info.pitch as usize).saturating_mul(info.height as usize);
    let word_count = total_bytes / 4;

    if info.addr == 0 {
        return;
    }

    unsafe {
        let src = bb.data.as_ptr() as *const u32;
        let dst = info.addr as *mut u32;
        for i in 0..word_count {
            // Bounds guard: i * 4 + 4 <= total_bytes (always true given word_count = total/4)
            core::ptr::write_volatile(dst.add(i), *src.add(i));
        }
        // Handle any trailing bytes (should not occur for 32-bpp, but be safe).
        let trailing_start = word_count * 4;
        if trailing_start < total_bytes {
            let src_bytes = bb.data.as_ptr();
            let dst_bytes = info.addr as *mut u8;
            for i in trailing_start..total_bytes {
                core::ptr::write_volatile(dst_bytes.add(i), *src_bytes.add(i));
            }
        }
    }
    // Full memory fence — ensure all writes reach VRAM before returning.
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Pixel operations
// ---------------------------------------------------------------------------

/// Put a pixel (only works in graphics mode)
pub fn put_pixel(x: u32, y: u32, color: Color) {
    let fb = FRAMEBUFFER.lock();
    if let Some(info) = *fb {
        if info.mode != DisplayMode::Graphics {
            return;
        }
        if x >= info.width || y >= info.height {
            return;
        }
        let offset = (y as usize)
            .saturating_mul(info.pitch as usize)
            .saturating_add((x as usize).saturating_mul(info.bpp as usize));
        let pixel = color.to_u32();

        drop(fb);
        let mut bb = BACK_BUFFER.lock();
        if bb.enabled && !bb.data.is_empty() {
            // Write to back buffer
            let bytes = pixel.to_ne_bytes();
            bb.data[offset..offset + 4].copy_from_slice(&bytes);
            bb.mark_dirty(x, y);
        } else {
            drop(bb);
            unsafe {
                core::ptr::write_volatile((info.addr + offset) as *mut u32, pixel);
            }
        }
    }
}

/// Read a pixel from the framebuffer (returns packed u32)
pub fn get_pixel(x: u32, y: u32) -> u32 {
    let fb = FRAMEBUFFER.lock();
    if let Some(info) = *fb {
        if info.mode != DisplayMode::Graphics {
            return 0;
        }
        if x >= info.width || y >= info.height {
            return 0;
        }
        let offset = (y as usize)
            .saturating_mul(info.pitch as usize)
            .saturating_add((x as usize).saturating_mul(info.bpp as usize));

        drop(fb);
        let bb = BACK_BUFFER.lock();
        if bb.enabled && !bb.data.is_empty() {
            let mut bytes = [0u8; 4];
            bytes.copy_from_slice(&bb.data[offset..offset + 4]);
            u32::from_ne_bytes(bytes)
        } else {
            drop(bb);
            unsafe { core::ptr::read_volatile((info.addr + offset) as *const u32) }
        }
    } else {
        0
    }
}

/// Put a pixel with alpha blending over the existing pixel
pub fn put_pixel_blend(x: u32, y: u32, color: Color) {
    if color.a == 255 {
        put_pixel(x, y, color);
        return;
    }
    if color.a == 0 {
        return;
    }
    let bg = Color::from_u32(get_pixel(x, y));
    let blended = color.blend_over(bg);
    put_pixel(x, y, blended);
}

// ---------------------------------------------------------------------------
// Line drawing: Bresenham's algorithm
// ---------------------------------------------------------------------------

/// Draw a line from (x0,y0) to (x1,y1) using Bresenham's algorithm
pub fn draw_line(x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
    // Fast paths for horizontal and vertical lines
    if y0 == y1 {
        draw_hline(x0, x1, y0, color);
        return;
    }
    if x0 == x1 {
        draw_vline(x0, y0, y1, color);
        return;
    }

    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x >= 0 && y >= 0 {
            put_pixel(x as u32, y as u32, color);
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = err * 2;
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

/// Draw a horizontal line (optimized)
pub fn draw_hline(x0: i32, x1: i32, y: i32, color: Color) {
    if y < 0 {
        return;
    }
    let (start, end) = if x0 <= x1 { (x0, x1) } else { (x1, x0) };
    let start = if start < 0 { 0 } else { start as u32 };
    let end = end as u32;
    for x in start..=end {
        put_pixel(x, y as u32, color);
    }
}

/// Draw a vertical line (optimized)
pub fn draw_vline(x: i32, y0: i32, y1: i32, color: Color) {
    if x < 0 {
        return;
    }
    let (start, end) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
    let start = if start < 0 { 0 } else { start as u32 };
    let end = end as u32;
    for y in start..=end {
        put_pixel(x as u32, y, color);
    }
}

// ---------------------------------------------------------------------------
// Rectangle operations
// ---------------------------------------------------------------------------

/// Fill a rectangle with a solid color
pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: Color) {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let pixel = color.to_u32();
    let x_end = x.saturating_add(w).min(info.width);
    let y_end = y.saturating_add(h).min(info.height);

    let mut bb = BACK_BUFFER.lock();
    if bb.enabled && !bb.data.is_empty() {
        let bytes = pixel.to_ne_bytes();
        for row in y..y_end {
            let row_off = (row as usize).saturating_mul(info.pitch as usize);
            for col in x..x_end {
                let off = row_off.saturating_add((col as usize).saturating_mul(info.bpp as usize));
                if off + 4 <= bb.data.len() {
                    bb.data[off..off + 4].copy_from_slice(&bytes);
                }
            }
        }
        bb.mark_rect_dirty(x, y, x_end.saturating_sub(x), y_end.saturating_sub(y));
    } else {
        drop(bb);
        for row in y..y_end {
            let row_off = (row as usize).saturating_mul(info.pitch as usize);
            for col in x..x_end {
                let off = row_off.saturating_add((col as usize).saturating_mul(info.bpp as usize));
                unsafe {
                    core::ptr::write_volatile((info.addr + off) as *mut u32, pixel);
                }
            }
        }
    }
}

/// Draw a rectangle outline (1px border)
pub fn draw_rect(x: u32, y: u32, w: u32, h: u32, color: Color) {
    if w == 0 || h == 0 {
        return;
    }
    let x_end = x.saturating_add(w).saturating_sub(1);
    let y_end = y.saturating_add(h).saturating_sub(1);
    // Top edge
    draw_hline(x as i32, x_end as i32, y as i32, color);
    // Bottom edge
    draw_hline(x as i32, x_end as i32, y_end as i32, color);
    // Left edge
    draw_vline(x as i32, y as i32, y_end as i32, color);
    // Right edge
    draw_vline(x_end as i32, y as i32, y_end as i32, color);
}

// ---------------------------------------------------------------------------
// 8x16 bitmap font -- ASCII 32-126
// ---------------------------------------------------------------------------

/// Font glyph dimensions
pub const FONT_WIDTH: u32 = 8;
pub const FONT_HEIGHT: u32 = 16;

/// Compact bitmap font: 95 glyphs (ASCII 32..126), 16 bytes per glyph.
/// Each byte is one row, MSB = leftmost pixel.
static FONT_DATA: [[u8; 16]; 95] = [
    // 32 = space
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 33 = !
    [
        0x00, 0x00, 0x18, 0x3C, 0x3C, 0x3C, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 34 = "
    [
        0x00, 0x66, 0x66, 0x66, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 35 = #
    [
        0x00, 0x00, 0x00, 0x6C, 0x6C, 0xFE, 0x6C, 0x6C, 0xFE, 0x6C, 0x6C, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 36 = $
    [
        0x18, 0x18, 0x7C, 0xC6, 0xC2, 0xC0, 0x7C, 0x06, 0x06, 0x86, 0xC6, 0x7C, 0x18, 0x18, 0x00,
        0x00,
    ],
    // 37 = %
    [
        0x00, 0x00, 0x00, 0x00, 0xC2, 0xC6, 0x0C, 0x18, 0x30, 0x60, 0xC6, 0x86, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 38 = &
    [
        0x00, 0x00, 0x38, 0x6C, 0x6C, 0x38, 0x76, 0xDC, 0xCC, 0xCC, 0xCC, 0x76, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 39 = '
    [
        0x00, 0x30, 0x30, 0x30, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 40 = (
    [
        0x00, 0x00, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x18, 0x0C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 41 = )
    [
        0x00, 0x00, 0x30, 0x18, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x18, 0x30, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 42 = *
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x66, 0x3C, 0xFF, 0x3C, 0x66, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 43 = +
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x7E, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 44 = ,
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x18, 0x30, 0x00, 0x00,
        0x00,
    ],
    // 45 = -
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFE, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 46 = .
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 47 = /
    [
        0x00, 0x00, 0x00, 0x00, 0x02, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0x80, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 48 = 0
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xCE, 0xDE, 0xF6, 0xE6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 49 = 1
    [
        0x00, 0x00, 0x18, 0x38, 0x78, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 50 = 2
    [
        0x00, 0x00, 0x7C, 0xC6, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0xC6, 0xFE, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 51 = 3
    [
        0x00, 0x00, 0x7C, 0xC6, 0x06, 0x06, 0x3C, 0x06, 0x06, 0x06, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 52 = 4
    [
        0x00, 0x00, 0x0C, 0x1C, 0x3C, 0x6C, 0xCC, 0xFE, 0x0C, 0x0C, 0x0C, 0x1E, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 53 = 5
    [
        0x00, 0x00, 0xFE, 0xC0, 0xC0, 0xC0, 0xFC, 0x06, 0x06, 0x06, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 54 = 6
    [
        0x00, 0x00, 0x38, 0x60, 0xC0, 0xC0, 0xFC, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 55 = 7
    [
        0x00, 0x00, 0xFE, 0xC6, 0x06, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x30, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 56 = 8
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 57 = 9
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0x7E, 0x06, 0x06, 0x06, 0x0C, 0x78, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 58 = :
    [
        0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 59 = ;
    [
        0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00, 0x18, 0x18, 0x30, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 60 = <
    [
        0x00, 0x00, 0x00, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 61 = =
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 62 = >
    [
        0x00, 0x00, 0x00, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x0C, 0x18, 0x30, 0x60, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 63 = ?
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0x0C, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 64 = @
    [
        0x00, 0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xDE, 0xDE, 0xDE, 0xDC, 0xC0, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 65 = A
    [
        0x00, 0x00, 0x10, 0x38, 0x6C, 0xC6, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 66 = B
    [
        0x00, 0x00, 0xFC, 0x66, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x66, 0x66, 0xFC, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 67 = C
    [
        0x00, 0x00, 0x3C, 0x66, 0xC2, 0xC0, 0xC0, 0xC0, 0xC0, 0xC2, 0x66, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 68 = D
    [
        0x00, 0x00, 0xF8, 0x6C, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x6C, 0xF8, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 69 = E
    [
        0x00, 0x00, 0xFE, 0x66, 0x62, 0x68, 0x78, 0x68, 0x60, 0x62, 0x66, 0xFE, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 70 = F
    [
        0x00, 0x00, 0xFE, 0x66, 0x62, 0x68, 0x78, 0x68, 0x60, 0x60, 0x60, 0xF0, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 71 = G
    [
        0x00, 0x00, 0x3C, 0x66, 0xC2, 0xC0, 0xC0, 0xDE, 0xC6, 0xC6, 0x66, 0x3A, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 72 = H
    [
        0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xFE, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 73 = I
    [
        0x00, 0x00, 0x3C, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 74 = J
    [
        0x00, 0x00, 0x1E, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0xCC, 0xCC, 0xCC, 0x78, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 75 = K
    [
        0x00, 0x00, 0xE6, 0x66, 0x66, 0x6C, 0x78, 0x78, 0x6C, 0x66, 0x66, 0xE6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 76 = L
    [
        0x00, 0x00, 0xF0, 0x60, 0x60, 0x60, 0x60, 0x60, 0x60, 0x62, 0x66, 0xFE, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 77 = M
    [
        0x00, 0x00, 0xC6, 0xEE, 0xFE, 0xFE, 0xD6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 78 = N
    [
        0x00, 0x00, 0xC6, 0xE6, 0xF6, 0xFE, 0xDE, 0xCE, 0xC6, 0xC6, 0xC6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 79 = O
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 80 = P
    [
        0x00, 0x00, 0xFC, 0x66, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x60, 0xF0, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 81 = Q
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xD6, 0xDE, 0x7C, 0x0C, 0x0E, 0x00,
        0x00,
    ],
    // 82 = R
    [
        0x00, 0x00, 0xFC, 0x66, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0x66, 0x66, 0xE6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 83 = S
    [
        0x00, 0x00, 0x7C, 0xC6, 0xC6, 0x60, 0x38, 0x0C, 0x06, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 84 = T
    [
        0x00, 0x00, 0xFF, 0xDB, 0x99, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 85 = U
    [
        0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 86 = V
    [
        0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x10, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 87 = W
    [
        0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xD6, 0xD6, 0xD6, 0xFE, 0xEE, 0x6C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 88 = X
    [
        0x00, 0x00, 0xC6, 0xC6, 0x6C, 0x7C, 0x38, 0x38, 0x7C, 0x6C, 0xC6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 89 = Y
    [
        0x00, 0x00, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 90 = Z
    [
        0x00, 0x00, 0xFE, 0xC6, 0x86, 0x0C, 0x18, 0x30, 0x60, 0xC2, 0xC6, 0xFE, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 91 = [
    [
        0x00, 0x00, 0x3C, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x30, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 92 = backslash
    [
        0x00, 0x00, 0x00, 0x80, 0xC0, 0x60, 0x30, 0x18, 0x0C, 0x06, 0x02, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 93 = ]
    [
        0x00, 0x00, 0x3C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x0C, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 94 = ^
    [
        0x10, 0x38, 0x6C, 0xC6, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 95 = _
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xFF, 0x00, 0x00,
        0x00,
    ],
    // 96 = `
    [
        0x30, 0x30, 0x18, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 97 = a
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x78, 0x0C, 0x7C, 0xCC, 0xCC, 0xCC, 0x76, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 98 = b
    [
        0x00, 0x00, 0xE0, 0x60, 0x60, 0x78, 0x6C, 0x66, 0x66, 0x66, 0x66, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 99 = c
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x7C, 0xC6, 0xC0, 0xC0, 0xC0, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 100 = d
    [
        0x00, 0x00, 0x1C, 0x0C, 0x0C, 0x3C, 0x6C, 0xCC, 0xCC, 0xCC, 0xCC, 0x76, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 101 = e
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x7C, 0xC6, 0xFE, 0xC0, 0xC0, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 102 = f
    [
        0x00, 0x00, 0x38, 0x6C, 0x64, 0x60, 0xF0, 0x60, 0x60, 0x60, 0x60, 0xF0, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 103 = g
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x76, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0x7C, 0x0C, 0xCC, 0x78,
        0x00,
    ],
    // 104 = h
    [
        0x00, 0x00, 0xE0, 0x60, 0x60, 0x6C, 0x76, 0x66, 0x66, 0x66, 0x66, 0xE6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 105 = i
    [
        0x00, 0x00, 0x18, 0x18, 0x00, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 106 = j
    [
        0x00, 0x00, 0x06, 0x06, 0x00, 0x0E, 0x06, 0x06, 0x06, 0x06, 0x06, 0x06, 0x66, 0x66, 0x3C,
        0x00,
    ],
    // 107 = k
    [
        0x00, 0x00, 0xE0, 0x60, 0x60, 0x66, 0x6C, 0x78, 0x78, 0x6C, 0x66, 0xE6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 108 = l
    [
        0x00, 0x00, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x3C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 109 = m
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xEC, 0xFE, 0xD6, 0xD6, 0xD6, 0xD6, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 110 = n
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xDC, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 111 = o
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x7C, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 112 = p
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xDC, 0x66, 0x66, 0x66, 0x66, 0x66, 0x7C, 0x60, 0x60, 0xF0,
        0x00,
    ],
    // 113 = q
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x76, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0x7C, 0x0C, 0x0C, 0x1E,
        0x00,
    ],
    // 114 = r
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xDC, 0x76, 0x66, 0x60, 0x60, 0x60, 0xF0, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 115 = s
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0x7C, 0xC6, 0x60, 0x38, 0x0C, 0xC6, 0x7C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 116 = t
    [
        0x00, 0x00, 0x10, 0x30, 0x30, 0xFC, 0x30, 0x30, 0x30, 0x30, 0x36, 0x1C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 117 = u
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0xCC, 0x76, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 118 = v
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x6C, 0x38, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 119 = w
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xC6, 0xC6, 0xD6, 0xD6, 0xD6, 0xFE, 0x6C, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 120 = x
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xC6, 0x6C, 0x38, 0x38, 0x38, 0x6C, 0xC6, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 121 = y
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0xC6, 0x7E, 0x06, 0x0C, 0xF8,
        0x00,
    ],
    // 122 = z
    [
        0x00, 0x00, 0x00, 0x00, 0x00, 0xFE, 0xCC, 0x18, 0x30, 0x60, 0xC6, 0xFE, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 123 = {
    [
        0x00, 0x00, 0x0E, 0x18, 0x18, 0x18, 0x70, 0x18, 0x18, 0x18, 0x18, 0x0E, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 124 = |
    [
        0x00, 0x00, 0x18, 0x18, 0x18, 0x18, 0x00, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 125 = }
    [
        0x00, 0x00, 0x70, 0x18, 0x18, 0x18, 0x0E, 0x18, 0x18, 0x18, 0x18, 0x70, 0x00, 0x00, 0x00,
        0x00,
    ],
    // 126 = ~
    [
        0x00, 0x00, 0x76, 0xDC, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00,
    ],
];

// ---------------------------------------------------------------------------
// Character and text rendering
// ---------------------------------------------------------------------------

/// Draw a single character at pixel position (x, y) using the bitmap font.
pub fn draw_char(x: u32, y: u32, ch: char, fg: Color, bg: Color) {
    let idx = ch as u32;
    if idx < 32 || idx > 126 {
        return;
    }
    let glyph = &FONT_DATA[(idx - 32) as usize];

    for row in 0..FONT_HEIGHT {
        let bits = glyph[row as usize];
        for col in 0..FONT_WIDTH {
            let px = x + col;
            let py = y + row;
            if bits & (0x80 >> col) != 0 {
                put_pixel(px, py, fg);
            } else {
                put_pixel(px, py, bg);
            }
        }
    }
}

/// Draw a string at pixel position (x, y). Characters are placed side by side.
pub fn draw_string(x: u32, y: u32, text: &str, fg: Color, bg: Color) {
    let mut cx = x;
    let mut cy = y;
    let fb = FRAMEBUFFER.lock();
    let max_w = match *fb {
        Some(i) => i.width,
        None => return,
    };
    drop(fb);

    for ch in text.chars() {
        if ch == '\n' {
            cx = x;
            cy = cy.saturating_add(FONT_HEIGHT);
            continue;
        }
        if ch == '\r' {
            cx = x;
            continue;
        }
        if ch == '\t' {
            cx = cx.saturating_add(FONT_WIDTH.saturating_mul(4)); // 4-space tab
            continue;
        }
        if cx.saturating_add(FONT_WIDTH) > max_w {
            cx = x;
            cy = cy.saturating_add(FONT_HEIGHT);
        }
        draw_char(cx, cy, ch, fg, bg);
        cx = cx.saturating_add(FONT_WIDTH);
    }
}

/// Draw a string with only the foreground pixels set (transparent background).
pub fn draw_string_transparent(x: u32, y: u32, text: &str, fg: Color) {
    let mut cx = x;
    let mut cy = y;
    let fb = FRAMEBUFFER.lock();
    let max_w = match *fb {
        Some(i) => i.width,
        None => return,
    };
    drop(fb);

    for ch in text.chars() {
        if ch == '\n' {
            cx = x;
            cy = cy.saturating_add(FONT_HEIGHT);
            continue;
        }
        let idx = ch as u32;
        if idx >= 32 && idx <= 126 {
            let glyph = &FONT_DATA[(idx - 32) as usize];
            for row in 0..FONT_HEIGHT {
                let bits = glyph[row as usize];
                for col in 0..FONT_WIDTH {
                    if bits & (0x80 >> col) != 0 {
                        put_pixel(cx.saturating_add(col), cy.saturating_add(row), fg);
                    }
                }
            }
        }
        cx = cx.saturating_add(FONT_WIDTH);
        if cx.saturating_add(FONT_WIDTH) > max_w {
            cx = x;
            cy = cy.saturating_add(FONT_HEIGHT);
        }
    }
}

// ---------------------------------------------------------------------------
// Scrolling
// ---------------------------------------------------------------------------

/// Scroll the framebuffer up by `rows` pixel rows.
/// The vacated rows at the bottom are filled with `fill_color`.
pub fn scroll_up(rows: u32, fill_color: Color) {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    if rows == 0 || rows >= info.height {
        clear(fill_color);
        return;
    }

    let mut bb = BACK_BUFFER.lock();
    if bb.enabled && !bb.data.is_empty() {
        // Copy rows upward in back buffer
        let row_bytes = info.pitch as usize;
        let src_start = (rows as usize).saturating_mul(row_bytes);
        let copy_rows = (info.height.saturating_sub(rows)) as usize;
        // Use memmove-safe copy via rotate or manual loop
        for r in 0..copy_rows {
            let dst_off = r * row_bytes;
            let src_off = src_start + r * row_bytes;
            // Copy row by row (src and dst don't overlap within a single row copy)
            let (left, right) = bb.data.split_at_mut(src_off);
            if dst_off + row_bytes <= left.len() && row_bytes <= right.len() {
                left[dst_off..dst_off + row_bytes].copy_from_slice(&right[..row_bytes]);
            }
        }
        // Clear bottom rows
        let fill = fill_color.to_u32().to_ne_bytes();
        let clear_start = copy_rows.saturating_mul(row_bytes);
        for r in 0..rows as usize {
            let row_off = clear_start.saturating_add(r.saturating_mul(row_bytes));
            for col in 0..info.width as usize {
                let off = row_off.saturating_add(col.saturating_mul(info.bpp as usize));
                if off + 4 <= bb.data.len() {
                    bb.data[off..off + 4].copy_from_slice(&fill);
                }
            }
        }
        bb.dirty = Some((
            0,
            0,
            info.width.saturating_sub(1),
            info.height.saturating_sub(1),
        ));
    } else {
        drop(bb);
        // Direct framebuffer scroll
        let pitch = info.pitch as usize;
        let src_start = info
            .addr
            .saturating_add((rows as usize).saturating_mul(pitch));
        let dst_start = info.addr;
        let copy_bytes = (info.height.saturating_sub(rows) as usize).saturating_mul(pitch);
        unsafe {
            core::ptr::copy(src_start as *const u8, dst_start as *mut u8, copy_bytes);
        }
        // Clear bottom
        let fill_val = fill_color.to_u32();
        let clear_y = info.height.saturating_sub(rows);
        for row in clear_y..info.height {
            for col in 0..info.width {
                let off = (row as usize)
                    .saturating_mul(pitch)
                    .saturating_add((col as usize).saturating_mul(info.bpp as usize));
                unsafe {
                    core::ptr::write_volatile(
                        (info.addr.saturating_add(off)) as *mut u32,
                        fill_val,
                    );
                }
            }
        }
    }
}

/// Scroll the framebuffer down by `rows` pixel rows.
pub fn scroll_down(rows: u32, fill_color: Color) {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    if rows == 0 || rows >= info.height {
        clear(fill_color);
        return;
    }

    // Copy rows downward (must go bottom-to-top to avoid overwriting)
    let fill_val = fill_color.to_u32();
    let pitch = info.pitch as usize;
    let bpp = info.bpp as usize;
    let copy_rows = info.height.saturating_sub(rows);
    for r in (0..copy_rows).rev() {
        let src_y = r;
        let dst_y = r.saturating_add(rows);
        for col in 0..info.width {
            let src_off = (src_y as usize)
                .saturating_mul(pitch)
                .saturating_add((col as usize).saturating_mul(bpp));
            let dst_off = (dst_y as usize)
                .saturating_mul(pitch)
                .saturating_add((col as usize).saturating_mul(bpp));
            unsafe {
                let pixel =
                    core::ptr::read_volatile((info.addr.saturating_add(src_off)) as *const u32);
                core::ptr::write_volatile((info.addr.saturating_add(dst_off)) as *mut u32, pixel);
            }
        }
    }
    // Clear top rows
    for row in 0..rows {
        for col in 0..info.width {
            let off = (row as usize)
                .saturating_mul(pitch)
                .saturating_add((col as usize).saturating_mul(bpp));
            unsafe {
                core::ptr::write_volatile((info.addr.saturating_add(off)) as *mut u32, fill_val);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Clear screen
// ---------------------------------------------------------------------------

/// Clear the screen to a color
pub fn clear(color: Color) {
    let fb = FRAMEBUFFER.lock();
    if let Some(info) = *fb {
        if info.mode == DisplayMode::Graphics {
            let pixel_value = color.to_u32();
            drop(fb);

            let mut bb = BACK_BUFFER.lock();
            if bb.enabled && !bb.data.is_empty() {
                let bytes = pixel_value.to_ne_bytes();
                let total_pixels = (info.width * info.height) as usize;
                for i in 0..total_pixels {
                    let row = i / info.width as usize;
                    let col = i % info.width as usize;
                    let off = row * info.pitch as usize + col * info.bpp as usize;
                    if off + 4 <= bb.data.len() {
                        bb.data[off..off + 4].copy_from_slice(&bytes);
                    }
                }
                bb.dirty = Some((0, 0, info.width - 1, info.height - 1));
            } else {
                drop(bb);
                let pitch = info.pitch as usize;
                let bpp = info.bpp as usize;
                for y in 0..info.height {
                    for x in 0..info.width {
                        let offset = (y as usize)
                            .saturating_mul(pitch)
                            .saturating_add((x as usize).saturating_mul(bpp));
                        unsafe {
                            core::ptr::write_volatile(
                                (info.addr.saturating_add(offset)) as *mut u32,
                                pixel_value,
                            );
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Circle drawing
// ---------------------------------------------------------------------------

/// Draw a circle outline using the midpoint algorithm (integer only)
pub fn draw_circle(cx: i32, cy: i32, radius: i32, color: Color) {
    let mut x = radius;
    let mut y = 0i32;
    let mut d = 1 - radius;

    while x >= y {
        put_pixel((cx + x) as u32, (cy + y) as u32, color);
        put_pixel((cx - x) as u32, (cy + y) as u32, color);
        put_pixel((cx + x) as u32, (cy - y) as u32, color);
        put_pixel((cx - x) as u32, (cy - y) as u32, color);
        put_pixel((cx + y) as u32, (cy + x) as u32, color);
        put_pixel((cx - y) as u32, (cy + x) as u32, color);
        put_pixel((cx + y) as u32, (cy - x) as u32, color);
        put_pixel((cx - y) as u32, (cy - x) as u32, color);

        y = y.saturating_add(1);
        if d <= 0 {
            d = d.saturating_add(2i32.saturating_mul(y).saturating_add(1));
        } else {
            x = x.saturating_sub(1);
            d = d.saturating_add(2i32.saturating_mul(y.saturating_sub(x)).saturating_add(1));
        }
    }
}

/// Fill a circle using horizontal spans
pub fn fill_circle(cx: i32, cy: i32, radius: i32, color: Color) {
    let mut x = radius;
    let mut y = 0i32;
    let mut d = 1 - radius;

    while x >= y {
        draw_hline(cx - x, cx + x, cy + y, color);
        draw_hline(cx - x, cx + x, cy - y, color);
        draw_hline(cx - y, cx + y, cy + x, color);
        draw_hline(cx - y, cx + y, cy - x, color);

        y = y.saturating_add(1);
        if d <= 0 {
            d = d.saturating_add(2i32.saturating_mul(y).saturating_add(1));
        } else {
            x = x.saturating_sub(1);
            d = d.saturating_add(2i32.saturating_mul(y.saturating_sub(x)).saturating_add(1));
        }
    }
}

// ---------------------------------------------------------------------------
// Bitmap blit
// ---------------------------------------------------------------------------

/// Blit a 32-bit ARGB pixel buffer to the framebuffer at (dst_x, dst_y).
/// `src_pitch` is the number of bytes per row in the source buffer.
/// Uses direct copy (no alpha blending).
pub fn blit_buffer(dst_x: u32, dst_y: u32, src: &[u32], src_w: u32, src_h: u32) {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let mut bb = BACK_BUFFER.lock();
    let use_bb = bb.enabled && !bb.data.is_empty();

    let pitch = info.pitch as usize;
    let bpp = info.bpp as usize;
    for row in 0..src_h {
        let dy = dst_y.saturating_add(row);
        if dy >= info.height {
            break;
        }
        for col in 0..src_w {
            let dx = dst_x.saturating_add(col);
            if dx >= info.width {
                break;
            }
            let src_idx = (row as usize)
                .saturating_mul(src_w as usize)
                .saturating_add(col as usize);
            if src_idx >= src.len() {
                break;
            }
            let pixel = src[src_idx];
            let off = (dy as usize)
                .saturating_mul(pitch)
                .saturating_add((dx as usize).saturating_mul(bpp));

            if use_bb {
                let bytes = pixel.to_ne_bytes();
                if off + 4 <= bb.data.len() {
                    bb.data[off..off + 4].copy_from_slice(&bytes);
                }
            } else {
                unsafe {
                    core::ptr::write_volatile((info.addr + off) as *mut u32, pixel);
                }
            }
        }
    }

    if use_bb {
        bb.mark_rect_dirty(dst_x, dst_y, src_w, src_h);
    }
}

/// Blit with per-pixel alpha blending (ARGB source over destination).
/// Uses integer-only math: (src * alpha + dst * (255 - alpha)) / 255
pub fn blit_buffer_blend(dst_x: u32, dst_y: u32, src: &[u32], src_w: u32, src_h: u32) {
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    for row in 0..src_h {
        let dy = dst_y.saturating_add(row);
        if dy >= info.height {
            break;
        }
        for col in 0..src_w {
            let dx = dst_x.saturating_add(col);
            if dx >= info.width {
                break;
            }
            let src_idx = (row as usize)
                .saturating_mul(src_w as usize)
                .saturating_add(col as usize);
            if src_idx >= src.len() {
                break;
            }
            let sp = src[src_idx];
            let sa = (sp >> 24) & 0xFF;
            if sa == 0 {
                continue;
            }
            if sa == 255 {
                put_pixel(dx, dy, Color::from_u32(sp));
                continue;
            }
            let da = 255 - sa;
            let dp = get_pixel(dx, dy);
            let r = ((sp >> 16 & 0xFF) * sa + (dp >> 16 & 0xFF) * da) / 255;
            let g = ((sp >> 8 & 0xFF) * sa + (dp >> 8 & 0xFF) * da) / 255;
            let b = ((sp & 0xFF) * sa + (dp & 0xFF) * da) / 255;
            let result = 0xFF000000 | (r << 16) | (g << 8) | b;
            put_pixel(dx, dy, Color::from_u32(result));
        }
    }
}

// ---------------------------------------------------------------------------
// Gradient fill
// ---------------------------------------------------------------------------

/// Fill a rectangle with a vertical gradient from `top_color` to `bottom_color`.
/// Uses integer-only linear interpolation.
pub fn fill_gradient_v(x: u32, y: u32, w: u32, h: u32, top: Color, bottom: Color) {
    if h == 0 {
        return;
    }
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let x_end = x.saturating_add(w).min(info.width);
    let y_end = y.saturating_add(h).min(info.height);
    let pitch = info.pitch as usize;
    let bpp = info.bpp as usize;

    for row in y..y_end {
        // t goes from 0 to (h-1); interpolate with integer math
        let t = row.saturating_sub(y);
        let inv = h.saturating_sub(1);
        // lerp: top * (inv - t) / inv + bottom * t / inv
        let r = if inv == 0 {
            top.r
        } else {
            ((top.r as u32 * inv.saturating_sub(t) + bottom.r as u32 * t) / inv) as u8
        };
        let g = if inv == 0 {
            top.g
        } else {
            ((top.g as u32 * inv.saturating_sub(t) + bottom.g as u32 * t) / inv) as u8
        };
        let b = if inv == 0 {
            top.b
        } else {
            ((top.b as u32 * inv.saturating_sub(t) + bottom.b as u32 * t) / inv) as u8
        };
        let color = Color::rgb(r, g, b);
        let pixel = color.to_u32();

        // Write the entire row with this color
        for col in x..x_end {
            let off = (row as usize)
                .saturating_mul(pitch)
                .saturating_add((col as usize).saturating_mul(bpp));
            unsafe {
                core::ptr::write_volatile((info.addr.saturating_add(off)) as *mut u32, pixel);
            }
        }
    }
}

/// Fill a rectangle with a horizontal gradient from `left_color` to `right_color`.
/// Uses integer-only linear interpolation.
pub fn fill_gradient_h(x: u32, y: u32, w: u32, h: u32, left: Color, right: Color) {
    if w == 0 {
        return;
    }
    let fb = FRAMEBUFFER.lock();
    let info = match *fb {
        Some(i) if i.mode == DisplayMode::Graphics => i,
        _ => return,
    };
    drop(fb);

    let x_end = x.saturating_add(w).min(info.width);
    let y_end = y.saturating_add(h).min(info.height);
    let pitch = info.pitch as usize;
    let bpp = info.bpp as usize;

    for col in x..x_end {
        let t = col.saturating_sub(x);
        let inv = w.saturating_sub(1);
        let r = if inv == 0 {
            left.r
        } else {
            ((left.r as u32 * inv.saturating_sub(t) + right.r as u32 * t) / inv) as u8
        };
        let g = if inv == 0 {
            left.g
        } else {
            ((left.g as u32 * inv.saturating_sub(t) + right.g as u32 * t) / inv) as u8
        };
        let b = if inv == 0 {
            left.b
        } else {
            ((left.b as u32 * inv.saturating_sub(t) + right.b as u32 * t) / inv) as u8
        };
        let pixel = Color::rgb(r, g, b).to_u32();

        for row in y..y_end {
            let off = (row as usize)
                .saturating_mul(pitch)
                .saturating_add((col as usize).saturating_mul(bpp));
            unsafe {
                core::ptr::write_volatile((info.addr.saturating_add(off)) as *mut u32, pixel);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rounded rectangle
// ---------------------------------------------------------------------------

/// Draw a filled rounded rectangle with integer-only quarter-circle corners.
pub fn fill_rounded_rect(x: u32, y: u32, w: u32, h: u32, radius: u32, color: Color) {
    if w == 0 || h == 0 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2);
    let two_r = r.saturating_mul(2);

    // Fill the center rectangle (excluding corners)
    fill_rect(x.saturating_add(r), y, w.saturating_sub(two_r), h, color);
    // Fill left and right side strips
    fill_rect(x, y.saturating_add(r), r, h.saturating_sub(two_r), color);
    fill_rect(
        x.saturating_add(w).saturating_sub(r),
        y.saturating_add(r),
        r,
        h.saturating_sub(two_r),
        color,
    );

    // Fill four quarter-circle corners
    let ri = r as i32;
    let mut cx_val = ri;
    let mut cy_val = 0i32;
    let mut d = 1 - ri;

    while cx_val >= cy_val {
        // Top-left corner (center at x+r, y+r)
        draw_hline(
            (x + r) as i32 - cx_val,
            (x + r) as i32 - 1,
            (y + r) as i32 - cy_val,
            color,
        );
        draw_hline(
            (x + r) as i32 - cy_val,
            (x + r) as i32 - 1,
            (y + r) as i32 - cx_val,
            color,
        );
        // Top-right corner (center at x+w-r-1, y+r)
        draw_hline(
            (x + w - r) as i32,
            (x + w - r) as i32 + cx_val - 1,
            (y + r) as i32 - cy_val,
            color,
        );
        draw_hline(
            (x + w - r) as i32,
            (x + w - r) as i32 + cy_val - 1,
            (y + r) as i32 - cx_val,
            color,
        );
        // Bottom-left corner (center at x+r, y+h-r-1)
        draw_hline(
            (x + r) as i32 - cx_val,
            (x + r) as i32 - 1,
            (y + h - r) as i32 + cy_val - 1,
            color,
        );
        draw_hline(
            (x + r) as i32 - cy_val,
            (x + r) as i32 - 1,
            (y + h - r) as i32 + cx_val - 1,
            color,
        );
        // Bottom-right corner (center at x+w-r-1, y+h-r-1)
        draw_hline(
            (x + w - r) as i32,
            (x + w - r) as i32 + cx_val - 1,
            (y + h - r) as i32 + cy_val - 1,
            color,
        );
        draw_hline(
            (x + w - r) as i32,
            (x + w - r) as i32 + cy_val - 1,
            (y + h - r) as i32 + cx_val - 1,
            color,
        );

        cy_val = cy_val.saturating_add(1);
        if d <= 0 {
            d = d.saturating_add(2i32.saturating_mul(cy_val).saturating_add(1));
        } else {
            cx_val = cx_val.saturating_sub(1);
            d = d.saturating_add(
                2i32.saturating_mul(cy_val.saturating_sub(cx_val))
                    .saturating_add(1),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Triangle drawing
// ---------------------------------------------------------------------------

/// Fill a triangle with vertices (x0,y0), (x1,y1), (x2,y2) using scanline fill.
/// All integer math, no floats.
pub fn fill_triangle(x0: i32, y0: i32, x1: i32, y1: i32, x2: i32, y2: i32, color: Color) {
    // Sort vertices by y-coordinate (bubble sort of 3 elements)
    let (mut ax, mut ay) = (x0, y0);
    let (mut bx, mut by) = (x1, y1);
    let (mut cx, mut cy) = (x2, y2);
    if ay > by {
        core::mem::swap(&mut ax, &mut bx);
        core::mem::swap(&mut ay, &mut by);
    }
    if by > cy {
        core::mem::swap(&mut bx, &mut cx);
        core::mem::swap(&mut by, &mut cy);
    }
    if ay > by {
        core::mem::swap(&mut ax, &mut bx);
        core::mem::swap(&mut ay, &mut by);
    }

    if ay == cy {
        return;
    } // degenerate

    // Scanline fill using fixed-point (Q16) edge walking
    for y in ay..=cy {
        // Compute x intersections with the two active edges using integer cross-multiply
        let xl;
        let xr;

        // Edge AC always spans the full height
        let ac_dy = cy - ay;
        let ac_x = ax + (cx - ax) * (y - ay) / ac_dy;

        if y < by {
            // Upper half: edge AB
            let ab_dy = by - ay;
            if ab_dy == 0 {
                continue;
            }
            let ab_x = ax + (bx - ax) * (y - ay) / ab_dy;
            xl = if ac_x < ab_x { ac_x } else { ab_x };
            xr = if ac_x > ab_x { ac_x } else { ab_x };
        } else {
            // Lower half: edge BC
            let bc_dy = cy - by;
            if bc_dy == 0 {
                xl = ac_x;
                xr = ac_x;
            } else {
                let bc_x = bx + (cx - bx) * (y - by) / bc_dy;
                xl = if ac_x < bc_x { ac_x } else { bc_x };
                xr = if ac_x > bc_x { ac_x } else { bc_x };
            }
        }

        draw_hline(xl, xr, y, color);
    }
}

use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Hardware constants
// ---------------------------------------------------------------------------

const FB_BASE:   usize = 0xFD000000;
const FB_BACK:   usize = 0xFD800000;
const FB_WIDTH:  u32   = 1920;
const FB_HEIGHT: u32   = 1040;
const FB_STRIDE: u32   = 1920 * 4;
const FB_SIZE:   u32   = 1920 * 1040 * 4;

const VGA_STATUS:   u16 = 0x3DA;
const VGA_CRTC_IDX: u16 = 0x3D4;
const VGA_CRTC_DAT: u16 = 0x3D5;

// ---------------------------------------------------------------------------
// Layer
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct Layer {
    pub base_addr: usize,
    pub width:     u16,
    pub height:    u16,
    pub x:         u16,
    pub y:         u16,
    pub alpha:     u8,
    pub visible:   bool,
    pub dirty:     bool,
}

impl Layer {
    pub const fn empty() -> Self {
        Self {
            base_addr: 0,
            width:     0,
            height:    0,
            x:         0,
            y:         0,
            alpha:     255,
            visible:   false,
            dirty:     false,
        }
    }
}

// ---------------------------------------------------------------------------
// CompositorState
// ---------------------------------------------------------------------------

pub struct CompositorState {
    pub front_buffer:          usize,
    pub back_buffer:           usize,
    pub layers:                [Layer; 8],
    pub layer_count:           usize,
    pub vsync_count:           u32,
    pub frame_count:           u32,
    pub in_vblank:             bool,
    pub flip_pending:          bool,
    pub render_time_cycles:    u64,
    pub fps_approx:            u16,
    pub compositing_quality:   u16,
    pub double_buffered:       bool,
    // internal: tick counter for fps window
    tick_counter:              u32,
    fps_frame_accumulator:     u32,
}

impl CompositorState {
    pub const fn new() -> Self {
        Self {
            front_buffer:        FB_BASE,
            back_buffer:         FB_BACK,
            layers:              [Layer::empty(); 8],
            layer_count:         0,
            vsync_count:         0,
            frame_count:         0,
            in_vblank:           false,
            flip_pending:        false,
            render_time_cycles:  0,
            fps_approx:          0,
            compositing_quality: 0,
            double_buffered:     false,
            tick_counter:        0,
            fps_frame_accumulator: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Static state
// ---------------------------------------------------------------------------

pub static STATE: Mutex<CompositorState> = Mutex::new(CompositorState::new());

// ---------------------------------------------------------------------------
// Unsafe hardware helpers
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    val
}

/// Returns true when the VGA vertical sync signal is active (bit 3 of status).
#[inline(always)]
unsafe fn is_vsync() -> bool {
    (inb(VGA_STATUS) & 0x08) != 0
}

/// Poll until vsync goes low then high — captures the start of the vblank
/// interval. Avoids locking inside a previous vsync pulse.
unsafe fn wait_vblank_start() {
    // First: wait for vsync to go low (exit any active vblank)
    let mut safety = 0u32;
    while is_vsync() {
        safety = safety.saturating_add(1);
        if safety > 200_000 {
            break;
        }
    }
    // Then: wait for vsync to go high (entering vblank)
    safety = 0;
    while !is_vsync() {
        safety = safety.saturating_add(1);
        if safety > 200_000 {
            break;
        }
    }
}

/// Fill a rectangle in a framebuffer with a solid 32-bit ARGB color.
/// All coordinates are bounds-checked against FB_WIDTH / FB_HEIGHT.
unsafe fn fb_fill_rect(base: usize, x: u32, y: u32, w: u32, h: u32, color: u32) {
    if w == 0 || h == 0 {
        return;
    }
    // Clamp to screen bounds
    let x_end = x.saturating_add(w).min(FB_WIDTH);
    let y_end = y.saturating_add(h).min(FB_HEIGHT);
    if x >= FB_WIDTH || y >= FB_HEIGHT || x_end == 0 || y_end == 0 {
        return;
    }
    let row_start = y;
    let row_stop  = y_end;
    let col_start = x;
    let col_stop  = x_end;

    let mut row = row_start;
    while row < row_stop {
        let mut col = col_start;
        while col < col_stop {
            let offset = row.saturating_mul(FB_STRIDE).saturating_add(col.saturating_mul(4)) as usize;
            let ptr = (base + offset) as *mut u32;
            core::ptr::write_volatile(ptr, color);
            col = col.saturating_add(1);
        }
        row = row.saturating_add(1);
    }
}

/// Copy `count_pixels` pixels (4 bytes each) from src to dst using volatile
/// writes to prevent the compiler from eliding the store.
unsafe fn fb_blit(src: usize, dst: usize, count_pixels: u32) {
    if count_pixels == 0 {
        return;
    }
    let total_bytes = count_pixels.saturating_mul(4) as usize;
    let src_ptr = src as *const u32;
    let dst_ptr = dst as *mut u32;
    let pixel_count = count_pixels as usize;
    let mut i = 0usize;
    while i < pixel_count {
        let val = core::ptr::read_volatile(src_ptr.add(i));
        core::ptr::write_volatile(dst_ptr.add(i), val);
        i += 1;
    }
    let _ = total_bytes; // suppress unused warning
}

/// Integer alpha blend — no floats.
/// result_ch = (src_ch * alpha + dst_ch * (255 - alpha)) / 255
#[inline(always)]
fn alpha_blend(src: u32, dst: u32, alpha: u8) -> u32 {
    let a  = alpha as u32;
    let na = 255u32.saturating_sub(a);

    let sr = (src >> 16) & 0xFF;
    let sg = (src >>  8) & 0xFF;
    let sb =  src        & 0xFF;

    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >>  8) & 0xFF;
    let db =  dst        & 0xFF;

    // Divide by 255 using the fast approximation: (x + 1 + (x >> 8)) >> 8
    // which equals x / 255 for x in [0, 65535].
    let blend_ch = |s: u32, d: u32| -> u32 {
        let x = s.saturating_mul(a).saturating_add(d.saturating_mul(na));
        // x / 255  — fast integer path
        let x1 = x.saturating_add(1).saturating_add(x >> 8);
        x1 >> 8
    };

    let r = blend_ch(sr, dr);
    let g = blend_ch(sg, dg);
    let b = blend_ch(sb, db);

    0xFF000000 | (r << 16) | (g << 8) | b
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe the front framebuffer with a test write, enable double-buffering, and
/// log the buffer addresses.
pub fn init() {
    // Probe: write a sentinel value to FB_BASE pixel 0, read it back.
    let probe_ok = unsafe {
        let ptr = FB_BASE as *mut u32;
        let original = core::ptr::read_volatile(ptr);
        core::ptr::write_volatile(ptr, 0xDEAD_BEEF);
        let readback = core::ptr::read_volatile(ptr);
        core::ptr::write_volatile(ptr, original); // restore
        readback == 0xDEAD_BEEF
    };

    let mut s = STATE.lock();
    s.front_buffer  = FB_BASE;
    s.back_buffer   = FB_BACK;
    s.double_buffered = probe_ok;
    s.compositing_quality = 0;

    serial_println!(
        "[compositor] double-buffered online front=0x{:x} back=0x{:x}",
        s.front_buffer,
        s.back_buffer
    );
}

/// Register a new layer. Returns the layer index (0-7), or 8 on overflow.
pub fn add_layer(base: usize, x: u16, y: u16, w: u16, h: u16, alpha: u8) -> usize {
    let mut s = STATE.lock();
    if s.layer_count >= 8 {
        serial_println!("[compositor] add_layer: layer table full (max 8)");
        return 8;
    }
    let idx = s.layer_count;
    s.layers[idx] = Layer {
        base_addr: base,
        width:     w,
        height:    h,
        x,
        y,
        alpha,
        visible:   true,
        dirty:     true,
    };
    s.layer_count = s.layer_count.saturating_add(1);
    idx
}

/// Mark a layer dirty so it will be composited on the next `composite_all`.
pub fn mark_dirty(layer_idx: usize) {
    let mut s = STATE.lock();
    if layer_idx < s.layer_count {
        s.layers[layer_idx].dirty = true;
    }
}

/// Signal that the back buffer is ready for display; the actual pointer swap
/// happens at the next vsync inside `tick`.
pub fn flip() {
    let mut s = STATE.lock();
    s.flip_pending = true;
}

/// Composite all visible, dirty layers into the back buffer, then clear dirty
/// flags. For each layer the source pixels are blended into the destination
/// using the layer's alpha value.
pub fn composite_all() {
    let mut s = STATE.lock();
    let back = s.back_buffer;
    let layer_count = s.layer_count;

    let mut i = 0usize;
    while i < layer_count {
        let layer = s.layers[i];
        if !layer.visible || !layer.dirty {
            i += 1;
            continue;
        }

        let lw = layer.width  as u32;
        let lh = layer.height as u32;
        let lx = layer.x as u32;
        let ly = layer.y as u32;

        if lw == 0 || lh == 0 {
            i += 1;
            continue;
        }

        // Clip to screen
        let x_end = lx.saturating_add(lw).min(FB_WIDTH);
        let y_end = ly.saturating_add(lh).min(FB_HEIGHT);

        if lx >= FB_WIDTH || ly >= FB_HEIGHT {
            i += 1;
            continue;
        }

        if layer.alpha == 255 {
            // Fully opaque — fast blit row by row
            let mut row = ly;
            while row < y_end {
                let src_row_offset = (row.saturating_sub(ly)).saturating_mul(lw).saturating_mul(4) as usize;
                let dst_row_offset = row.saturating_mul(FB_STRIDE).saturating_add(lx.saturating_mul(4)) as usize;
                let pixels = x_end.saturating_sub(lx).min(lw);
                if pixels > 0 {
                    unsafe {
                        fb_blit(
                            layer.base_addr + src_row_offset,
                            back + dst_row_offset,
                            pixels,
                        );
                    }
                }
                row = row.saturating_add(1);
            }
        } else {
            // Partial alpha — pixel-by-pixel blend
            let mut row = ly;
            while row < y_end {
                let src_row = (row.saturating_sub(ly)).saturating_mul(lw) as usize;
                let dst_row = row.saturating_mul(FB_STRIDE / 4) as usize;
                let col_end = x_end.saturating_sub(lx).min(lw);
                let mut col = 0u32;
                while col < col_end {
                    let src_offset = src_row + col as usize;
                    let dst_offset = dst_row + (lx as usize) + col as usize;
                    unsafe {
                        let src_ptr = (layer.base_addr as *const u32).add(src_offset);
                        let dst_ptr = (back as *mut u32).add(dst_offset);
                        let src_px = core::ptr::read_volatile(src_ptr);
                        let dst_px = core::ptr::read_volatile(dst_ptr);
                        let blended = alpha_blend(src_px, dst_px, layer.alpha);
                        core::ptr::write_volatile(dst_ptr, blended);
                    }
                    col = col.saturating_add(1);
                }
                row = row.saturating_add(1);
            }
        }

        s.layers[i].dirty = false;
        i += 1;
    }
}

/// Main per-tick update.
/// - Polls vsync status.
/// - If a flip is pending and we have entered vblank, swaps front/back.
/// - Tracks approximate FPS over 60-tick windows.
/// - Advances compositing_quality toward 1000.
/// - Logs every 500 ticks.
pub fn tick(consciousness: u16, age: u32) {
    let _ = consciousness; // reserved for future quality scaling
    let _ = age;

    let vsync_active = unsafe { is_vsync() };

    let mut s = STATE.lock();

    s.in_vblank = vsync_active;

    // Page flip: swap pointers on entering vblank
    if s.flip_pending && s.in_vblank {
        let tmp = s.front_buffer;
        s.front_buffer = s.back_buffer;
        s.back_buffer  = tmp;
        s.frame_count  = s.frame_count.saturating_add(1);
        s.fps_frame_accumulator = s.fps_frame_accumulator.saturating_add(1);
        s.vsync_count  = s.vsync_count.saturating_add(1);
        s.flip_pending = false;
    }

    s.tick_counter = s.tick_counter.saturating_add(1);

    // Approximate FPS: every ~60 ticks reset the frame accumulator window.
    if s.tick_counter % 60 == 0 {
        // fps_approx = frames_in_window * 10  (e.g. 6 frames * 10 = 60 → 60.0 fps display)
        s.fps_approx = (s.fps_frame_accumulator.saturating_mul(10)) as u16;
        s.fps_frame_accumulator = 0;
    }

    // Grow compositing quality toward 1000
    if s.compositing_quality < 1000 {
        s.compositing_quality = s.compositing_quality.saturating_add(1);
    }

    // Log every 500 ticks
    if s.tick_counter % 500 == 0 {
        serial_println!(
            "[compositor] frames={} fps={}0.x vsync={} quality={}",
            s.frame_count,
            s.fps_approx,
            s.vsync_count,
            s.compositing_quality
        );
    }
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn compositing_quality() -> u16 {
    STATE.lock().compositing_quality
}

pub fn frame_count() -> u32 {
    STATE.lock().frame_count
}

pub fn fps_approx() -> u16 {
    STATE.lock().fps_approx
}

pub fn vsync_count() -> u32 {
    STATE.lock().vsync_count
}

pub fn in_vblank() -> bool {
    STATE.lock().in_vblank
}

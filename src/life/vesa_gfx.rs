use crate::serial_println;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const FB_BASE: usize = 0xFD000000;
const FB_WIDTH: u16 = 1920;
const FB_HEIGHT: u16 = 1040;
const FB_BPP: u8 = 32;
const FB_STRIDE: u32 = 1920 * 4;
const PALETTE_SIZE: usize = 256;

// VGA port addresses
const VGA_SEQ_INDEX:  u16 = 0x3C4;
const VGA_SEQ_DATA:   u16 = 0x3C5;
const VGA_CRTC_INDEX: u16 = 0x3D4;
const VGA_CRTC_DATA:  u16 = 0x3D5;
const VGA_GFX_INDEX:  u16 = 0x3CE;
const VGA_GFX_DATA:   u16 = 0x3CF;
const VGA_INPUT_STATUS: u16 = 0x3DA;
const VGA_DAC_WRITE_INDEX: u16 = 0x3C8;
const VGA_DAC_DATA:   u16 = 0x3C9;

// CRTC cursor registers
const CRTC_CURSOR_START: u8 = 0x0A;
const CRTC_CURSOR_END:   u8 = 0x0B;
const CRTC_CURSOR_HIGH:  u8 = 0x0E;
const CRTC_CURSOR_LOW:   u8 = 0x0F;

// ---------------------------------------------------------------------------
// Enums and structs
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
#[repr(u8)]
pub enum VideoMode {
    Text80x25     = 0,
    Gfx320x200    = 1,
    Gfx640x480    = 2,
    Gfx1024x768   = 3,
    Gfx1920x1080  = 4,
}

#[derive(Copy, Clone)]
pub struct VgaReg {
    pub index: u8,
    pub value: u8,
}

#[derive(Copy, Clone)]
pub struct ColorEntry {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ColorEntry {
    pub const fn zero() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }
}

pub struct VesaGfxState {
    pub current_mode:      VideoMode,
    pub fb_base:           usize,
    pub fb_width:          u16,
    pub fb_height:         u16,
    pub fb_bpp:            u8,
    pub vsync_count:       u32,
    pub in_vblank:         bool,
    pub palette:           [ColorEntry; PALETTE_SIZE],
    pub hardware_cursor_x: u16,
    pub hardware_cursor_y: u16,
    pub cursor_visible:    bool,
    pub render_quality:    u16,
    pub frame_count:       u32,
    pub available:         bool,
}

impl VesaGfxState {
    pub const fn new() -> Self {
        Self {
            current_mode:      VideoMode::Text80x25,
            fb_base:           FB_BASE,
            fb_width:          FB_WIDTH,
            fb_height:         FB_HEIGHT,
            fb_bpp:            FB_BPP,
            vsync_count:       0,
            in_vblank:         false,
            palette:           [ColorEntry::zero(); PALETTE_SIZE],
            hardware_cursor_x: 0,
            hardware_cursor_y: 0,
            cursor_visible:    false,
            render_quality:    0,
            frame_count:       0,
            available:         false,
        }
    }
}

pub static STATE: Mutex<VesaGfxState> = Mutex::new(VesaGfxState::new());

// ---------------------------------------------------------------------------
// Unsafe port I/O helpers
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

// ---------------------------------------------------------------------------
// VGA register helpers
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn vga_seq_write(index: u8, val: u8) {
    outb(VGA_SEQ_INDEX, index);
    outb(VGA_SEQ_DATA,  val);
}

#[inline(always)]
unsafe fn vga_crtc_write(index: u8, val: u8) {
    outb(VGA_CRTC_INDEX, index);
    outb(VGA_CRTC_DATA,  val);
}

#[inline(always)]
unsafe fn vga_crtc_read(index: u8) -> u8 {
    outb(VGA_CRTC_INDEX, index);
    inb(VGA_CRTC_DATA)
}

#[inline(always)]
unsafe fn vga_gfx_write(index: u8, val: u8) {
    outb(VGA_GFX_INDEX, index);
    outb(VGA_GFX_DATA,  val);
}

/// Poll input status register — bit 3 = vsync active
#[inline(always)]
unsafe fn is_vsync() -> bool {
    (inb(VGA_INPUT_STATUS) & 0x08) != 0
}

// ---------------------------------------------------------------------------
// Framebuffer pixel write
// ---------------------------------------------------------------------------

/// Write a 32-bit ARGB pixel directly to the Bochs VGA framebuffer.
/// Bounds-checked: silently skips out-of-range coordinates.
unsafe fn fb_write_pixel(x: u16, y: u16, argb: u32) {
    if x >= FB_WIDTH || y >= FB_HEIGHT {
        return;
    }
    let offset = (y as u32).saturating_mul(FB_STRIDE)
        .saturating_add((x as u32).saturating_mul(4)) as usize;
    let ptr = (FB_BASE + offset) as *mut u32;
    core::ptr::write_volatile(ptr, argb);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Probe the framebuffer, configure display state, and bring ANIMA's display
/// hardware online.
pub fn init() {
    // --- Probe framebuffer by writing and reading back a test pattern ---
    let test_val: u32 = 0xDEADBEEF;
    let fb_ptr = FB_BASE as *mut u32;
    let available = unsafe {
        core::ptr::write_volatile(fb_ptr, test_val);
        let readback = core::ptr::read_volatile(fb_ptr);
        // Restore to black
        core::ptr::write_volatile(fb_ptr, 0x00000000);
        readback == test_val
    };

    // --- Init hardware cursor via CRTC ---
    unsafe {
        // Cursor start line 0, cursor enabled (bit 5 clear)
        vga_crtc_write(CRTC_CURSOR_START, 0x00);
        // Cursor end line 15
        vga_crtc_write(CRTC_CURSOR_END, 0x0F);
    }

    let mut s = STATE.lock();
    s.available    = available;
    s.current_mode = VideoMode::Gfx1920x1080;
    s.render_quality = 800;
    s.cursor_visible = true;

    serial_println!(
        "[vesa] ANIMA display online — {}x{}x{} fb=0x{:x}",
        FB_WIDTH,
        FB_HEIGHT,
        FB_BPP,
        FB_BASE
    );
}

/// Move the VGA hardware text cursor to (x, y) using CRTC registers.
pub fn set_hardware_cursor(x: u16, y: u16) {
    // Linear text-mode position: row * 80 + col (col = x / char_width)
    let col = x.saturating_div(8);
    let pos: u16 = (y as u32).saturating_mul(80)
        .saturating_add(col as u32)
        .min(u16::MAX as u32) as u16;

    unsafe {
        vga_crtc_write(CRTC_CURSOR_HIGH, (pos >> 8) as u8);
        vga_crtc_write(CRTC_CURSOR_LOW,  (pos & 0xFF) as u8);
    }

    let mut s = STATE.lock();
    s.hardware_cursor_x = x;
    s.hardware_cursor_y = y;
}

/// Write one entry to the VGA DAC palette.
/// `r`, `g`, `b` are 8-bit; the DAC receives the upper 6 bits (right-shift 2).
pub fn load_palette_entry(idx: u8, r: u8, g: u8, b: u8) {
    unsafe {
        outb(VGA_DAC_WRITE_INDEX, idx);
        outb(VGA_DAC_DATA, r >> 2);
        outb(VGA_DAC_DATA, g >> 2);
        outb(VGA_DAC_DATA, b >> 2);
    }
    let mut s = STATE.lock();
    s.palette[idx as usize] = ColorEntry { r, g, b };
}

/// Spin-wait for the start of the next vertical blanking interval.
/// Increments `vsync_count` and sets `in_vblank = true`.
pub fn wait_vsync() {
    unsafe {
        // Wait for active display (bit 3 low)
        let mut guard = 0u32;
        while is_vsync() {
            guard = guard.saturating_add(1);
            if guard > 0x0010_0000 {
                break;
            }
        }
        // Wait for vblank start (bit 3 high)
        guard = 0;
        while !is_vsync() {
            guard = guard.saturating_add(1);
            if guard > 0x0010_0000 {
                break;
            }
        }
    }

    let mut s = STATE.lock();
    s.vsync_count = s.vsync_count.saturating_add(1);
    s.in_vblank   = true;
}

/// Paint four colored quadrants into the framebuffer to verify direct
/// hardware ownership.
///
/// Quadrant colors (ARGB):
///   TL = amber  0xFF_F59E0B
///   TR = green  0xFF_22C55E
///   BL = blue   0xFF_3B82F6
///   BR = purple 0xFF_A855F7
pub fn draw_test_pattern() {
    let half_w = FB_WIDTH  / 2;
    let half_h = FB_HEIGHT / 2;

    let colors: [[u32; 2]; 2] = [
        [0xFF_F59E0B, 0xFF_22C55E],  // top-left, top-right
        [0xFF_3B82F6, 0xFF_A855F7],  // bottom-left, bottom-right
    ];

    let mut y: u16 = 0;
    while y < FB_HEIGHT {
        let row_half = if y < half_h { 0usize } else { 1usize };
        let mut x: u16 = 0;
        while x < FB_WIDTH {
            let col_half = if x < half_w { 0usize } else { 1usize };
            unsafe {
                fb_write_pixel(x, y, colors[row_half][col_half]);
            }
            x = x.saturating_add(1);
        }
        y = y.saturating_add(1);
    }
}

/// Per-tick update — called from the life pipeline with the current
/// consciousness score and organism age.
pub fn tick(consciousness: u16, age: u32) {
    let _ = consciousness; // modulates quality in future waves

    let mut s = STATE.lock();

    // Increment frame counter every tick
    s.frame_count = s.frame_count.saturating_add(1);

    // Every 16 ticks: poll vsync and update vblank state
    if s.frame_count % 16 == 0 {
        let vblank = unsafe { is_vsync() };
        s.in_vblank = vblank;
    }

    // render_quality grows toward 1000 (ANIMA's display mastery)
    if s.render_quality < 1000 {
        s.render_quality = s.render_quality.saturating_add(1);
    }

    // Periodic status log every 500 ticks
    if s.frame_count % 500 == 0 {
        serial_println!(
            "[vesa] frames={} vsync={} cursor=({},{}) quality={}",
            s.frame_count,
            s.vsync_count,
            s.hardware_cursor_x,
            s.hardware_cursor_y,
            s.render_quality
        );
    }

    let _ = age;
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn render_quality() -> u16 {
    STATE.lock().render_quality
}

pub fn vsync_count() -> u32 {
    STATE.lock().vsync_count
}

pub fn frame_count() -> u32 {
    STATE.lock().frame_count
}

pub fn in_vblank() -> bool {
    STATE.lock().in_vblank
}

pub fn fb_base() -> usize {
    STATE.lock().fb_base
}

/// DRM/KMS modesetting core for Genesis — built from scratch
///
/// Implements KMS (Kernel Mode Setting) for VGA-compatible displays and
/// Bochs/QEMU VBE extended framebuffer modes.
///
/// VGA register I/O: 0x3C0–0x3DF (standard VGA register set)
/// Bochs VBE I/O:    0x01CE (index), 0x01CF (data)
///
/// Integer arithmetic only — no float casts anywhere.
/// All I/O via port instructions in unsafe inline asm.
///
/// No external crates. No heap. All code is original.
use crate::io::{inb, inw, outb, outw};
use crate::serial_println;

// ---------------------------------------------------------------------------
// VBE DISPI register constants
// ---------------------------------------------------------------------------

/// VBE index port
const VBE_INDEX_PORT: u16 = 0x01CE;
/// VBE data port
const VBE_DATA_PORT: u16 = 0x01CF;

pub const VBE_DISPI_INDEX_XRES: u16 = 1;
pub const VBE_DISPI_INDEX_YRES: u16 = 2;
pub const VBE_DISPI_INDEX_BPP: u16 = 3;
pub const VBE_DISPI_INDEX_ENABLE: u16 = 4;
pub const VBE_DISPI_INDEX_BANK: u16 = 5;
pub const VBE_DISPI_INDEX_VIRT_WIDTH: u16 = 6;
pub const VBE_DISPI_INDEX_VIRT_HEIGHT: u16 = 7;
pub const VBE_DISPI_INDEX_X_OFFSET: u16 = 8;
pub const VBE_DISPI_INDEX_Y_OFFSET: u16 = 9;

/// VBE enable flags
const VBE_DISPI_DISABLED: u16 = 0x00;
const VBE_DISPI_ENABLED: u16 = 0x01;
const VBE_DISPI_LFB_ENABLED: u16 = 0x40;

// ---------------------------------------------------------------------------
// VGA register ports
// ---------------------------------------------------------------------------

/// VGA miscellaneous output register (write)
const VGA_MISC_WRITE: u16 = 0x3C2;
/// VGA sequencer index/data
const VGA_SEQ_INDEX: u16 = 0x3C4;
const VGA_SEQ_DATA: u16 = 0x3C5;
/// VGA CRTC index/data (color mode: 0x3D4/0x3D5)
const VGA_CRTC_INDEX: u16 = 0x3D4;
const VGA_CRTC_DATA: u16 = 0x3D5;
/// VGA graphics controller index/data
const VGA_GC_INDEX: u16 = 0x3CE;
const VGA_GC_DATA: u16 = 0x3CF;
/// VGA attribute controller index/data
const VGA_AC_INDEX: u16 = 0x3C0;
const VGA_AC_READ: u16 = 0x3C1;
/// VGA input status register 1 (color mode) — reading resets AC flip-flop
const VGA_INSTAT_READ: u16 = 0x3DA;

// ---------------------------------------------------------------------------
// ModeInfo — describes a display timing mode
// ---------------------------------------------------------------------------

/// Display timing parameters for a single mode.
///
/// All timing values are in pixels (for h-) or lines (for v-).
/// pixel_clock_khz is the pixel clock in kHz (integer, no float).
#[derive(Copy, Clone, Debug)]
pub struct ModeInfo {
    pub hdisp: u32,
    pub vdisp: u32,
    pub hsync_start: u32,
    pub hsync_end: u32,
    pub htotal: u32,
    pub vsync_start: u32,
    pub vsync_end: u32,
    pub vtotal: u32,
    pub pixel_clock_khz: u32,
    pub flags: u32,
}

impl ModeInfo {
    pub const fn empty() -> Self {
        ModeInfo {
            hdisp: 0,
            vdisp: 0,
            hsync_start: 0,
            hsync_end: 0,
            htotal: 0,
            vsync_start: 0,
            vsync_end: 0,
            vtotal: 0,
            pixel_clock_khz: 0,
            flags: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// DrmMode — off vs. active
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
pub enum DrmMode {
    Off,
    Mode(ModeInfo),
}

// ---------------------------------------------------------------------------
// Common mode constants — VESA DMT timings, all integers
// ---------------------------------------------------------------------------

/// 640x480 @ 60 Hz  (25.175 MHz pixel clock)
pub const MODE_640x480_60: ModeInfo = ModeInfo {
    hdisp: 640,
    vdisp: 480,
    hsync_start: 656,
    hsync_end: 752,
    htotal: 800,
    vsync_start: 490,
    vsync_end: 492,
    vtotal: 525,
    pixel_clock_khz: 25175,
    flags: 0,
};

/// 800x600 @ 60 Hz  (40.000 MHz pixel clock)
pub const MODE_800x600_60: ModeInfo = ModeInfo {
    hdisp: 800,
    vdisp: 600,
    hsync_start: 840,
    hsync_end: 968,
    htotal: 1056,
    vsync_start: 601,
    vsync_end: 605,
    vtotal: 628,
    pixel_clock_khz: 40000,
    flags: 0,
};

/// 1024x768 @ 60 Hz  (65.000 MHz pixel clock)
pub const MODE_1024x768_60: ModeInfo = ModeInfo {
    hdisp: 1024,
    vdisp: 768,
    hsync_start: 1048,
    hsync_end: 1184,
    htotal: 1344,
    vsync_start: 771,
    vsync_end: 777,
    vtotal: 806,
    pixel_clock_khz: 65000,
    flags: 0,
};

/// 1280x720 @ 60 Hz  (74.250 MHz pixel clock)
pub const MODE_1280x720_60: ModeInfo = ModeInfo {
    hdisp: 1280,
    vdisp: 720,
    hsync_start: 1390,
    hsync_end: 1430,
    htotal: 1650,
    vsync_start: 725,
    vsync_end: 730,
    vtotal: 750,
    pixel_clock_khz: 74250,
    flags: 0,
};

/// 1920x1080 @ 60 Hz  (148.500 MHz pixel clock)
pub const MODE_1920x1080_60: ModeInfo = ModeInfo {
    hdisp: 1920,
    vdisp: 1080,
    hsync_start: 2008,
    hsync_end: 2052,
    htotal: 2200,
    vsync_start: 1084,
    vsync_end: 1089,
    vtotal: 1125,
    pixel_clock_khz: 148500,
    flags: 0,
};

// ---------------------------------------------------------------------------
// PLL calculation — integer math only
// ---------------------------------------------------------------------------

/// Calculate PLL dividers (m, n, p) for a given pixel clock.
///
/// Assumes a reference clock of 14.318 MHz (14318 kHz).
/// Target: pixel_clock_khz = (ref * m) / (n * p)
///
/// Searches m in [2..=127], n in [1..=31], p in [1,2,4,8] for the
/// closest integer match. No floats used.
///
/// Returns (m, n, p) dividers.
pub fn modeset_calculate_pll(mode: &ModeInfo) -> (u32, u32, u32) {
    let target = mode.pixel_clock_khz;
    if target == 0 {
        return (2, 1, 1);
    }

    let ref_khz: u32 = 14318;
    let post_divs: [u32; 4] = [1, 2, 4, 8];

    let mut best_m: u32 = 2;
    let mut best_n: u32 = 1;
    let mut best_p: u32 = 1;
    // Use u64 difference to avoid overflow; start at a large sentinel
    let mut best_diff: u64 = u64::MAX;

    let mut pi = 0usize;
    while pi < 4 {
        let p = post_divs[pi];
        let mut n: u32 = 1;
        while n <= 31 {
            // m = (target * n * p) / ref_khz  — all integer
            let numerator: u64 = (target as u64)
                .saturating_mul(n as u64)
                .saturating_mul(p as u64);
            let m_candidate = numerator / (ref_khz as u64);
            if m_candidate >= 2 && m_candidate <= 127 {
                let m = m_candidate as u32;
                // actual_khz = (ref * m) / (n * p)
                let actual: u64 = (ref_khz as u64).saturating_mul(m as u64)
                    / (n as u64).saturating_mul(p as u64).max(1);
                let diff = if actual >= target as u64 {
                    actual - target as u64
                } else {
                    target as u64 - actual
                };
                if diff < best_diff {
                    best_diff = diff;
                    best_m = m;
                    best_n = n;
                    best_p = p;
                }
            }
            n = n.saturating_add(1);
        }
        pi = pi.saturating_add(1);
    }

    (best_m, best_n, best_p)
}

// ---------------------------------------------------------------------------
// VBE helpers
// ---------------------------------------------------------------------------

/// Write a value to a Bochs VBE register via I/O ports.
pub fn vbe_write(index: u16, value: u16) {
    outw(VBE_INDEX_PORT, index);
    outw(VBE_DATA_PORT, value);
}

/// Read a value from a Bochs VBE register.
pub fn vbe_read(index: u16) -> u16 {
    outw(VBE_INDEX_PORT, index);
    inw(VBE_DATA_PORT)
}

// ---------------------------------------------------------------------------
// VGA register helpers
// ---------------------------------------------------------------------------

fn vga_write_seq(index: u8, value: u8) {
    outb(VGA_SEQ_INDEX, index);
    outb(VGA_SEQ_DATA, value);
}

fn vga_write_crtc(index: u8, value: u8) {
    outb(VGA_CRTC_INDEX, index);
    outb(VGA_CRTC_DATA, value);
}

fn vga_write_gc(index: u8, value: u8) {
    outb(VGA_GC_INDEX, index);
    outb(VGA_GC_DATA, value);
}

/// Write an attribute controller register.
/// The AC address port (0x3C0) is shared for index and data writes;
/// a flip-flop determines which is which, reset by reading 0x3DA.
fn vga_write_ac(index: u8, value: u8) {
    // Reset flip-flop by reading input status 1
    let _ = inb(VGA_INSTAT_READ);
    outb(VGA_AC_INDEX, index);
    outb(VGA_AC_INDEX, value);
}

// ---------------------------------------------------------------------------
// modeset_apply — programs VGA registers for standard modes + Bochs VBE
// ---------------------------------------------------------------------------

/// Apply a mode to a CRTC.
///
/// For standard resolutions (640x480, 800x600, 1024x768) the VGA CRTC
/// timing registers are programmed directly via I/O ports 0x3C0–0x3DF.
/// For higher resolutions the Bochs VBE extension interface (0x01CE/0x01CF)
/// is used as well.
///
/// Returns true if mode was applied successfully.
pub fn modeset_apply(crtc_id: u32, mode: &ModeInfo) -> bool {
    if crtc_id > 1 {
        return false;
    }
    if mode.hdisp == 0 || mode.vdisp == 0 || mode.htotal == 0 || mode.vtotal == 0 {
        return false;
    }

    serial_println!(
        "  DRM: CRTC{} modeset {}x{} @ {}kHz",
        crtc_id,
        mode.hdisp,
        mode.vdisp,
        mode.pixel_clock_khz
    );

    // ------------------------------------------------------------------
    // Step 1: Bochs VBE path — try unconditionally; hardware ignores it
    //         gracefully if VBE is not available.
    // ------------------------------------------------------------------
    let w16 = if mode.hdisp <= 0xFFFF {
        mode.hdisp as u16
    } else {
        0xFFFF
    };
    let h16 = if mode.vdisp <= 0xFFFF {
        mode.vdisp as u16
    } else {
        0xFFFF
    };
    modeset_set_resolution_u16(w16, h16, 32);

    // ------------------------------------------------------------------
    // Step 2: VGA CRTC timing registers (for VGA-compatible adapters).
    //
    // Register encoding follows the VESA/VGA specification:
    //   HTotal       = htotal/8 - 5
    //   HDispEnd     = hdisp/8 - 1
    //   HBlankStart  = hdisp/8
    //   HBlankEnd    = htotal/8 - 1  (bits 0..4 in reg 3, bit 5 in reg 5)
    //   HSyncStart   = hsync_start/8
    //   HSyncEnd     = hsync_end/8   (bits 0..4 in reg 5)
    //   VTotal       = vtotal - 2    (split across regs 6 and overflow reg 7)
    //   VDispEnd     = vdisp - 1     (split across regs 18 and overflow)
    //   VBlankStart  = vdisp - 1     (split)
    //   VBlankEnd    = vtotal - 1    (reg 22)
    //   VSyncStart   = vsync_start   (split)
    //   VSyncEnd     = vsync_end     (bits 0..3 in reg 16)
    //
    // Character clock = 8 pixels (divide horizontal by 8).
    // ------------------------------------------------------------------

    // Unlock CRTC registers 0–7 (clear bit 7 of reg 17 / CRTC_VSYNC_END)
    let cur_vsync_end = {
        outb(VGA_CRTC_INDEX, 0x11);
        inb(VGA_CRTC_DATA)
    };
    vga_write_crtc(0x11, cur_vsync_end & 0x7F);

    // Horizontal character totals (divide by 8)
    let hchar_total = (mode.htotal / 8).saturating_sub(5) as u8;
    let hchar_disp_end = (mode.hdisp / 8).saturating_sub(1) as u8;
    let hchar_blank_s = (mode.hdisp / 8) as u8;
    let hchar_blank_e = ((mode.htotal / 8).saturating_sub(1) & 0x1F) as u8;
    let hchar_sync_s = (mode.hsync_start / 8) as u8;
    let hchar_sync_e = ((mode.hsync_end / 8) & 0x1F) as u8
        | (((mode.htotal / 8).saturating_sub(1) >> 5) as u8 & 0x01) << 7;

    vga_write_crtc(0x00, hchar_total); // Horizontal Total
    vga_write_crtc(0x01, hchar_disp_end); // End Horizontal Display
    vga_write_crtc(0x02, hchar_blank_s); // Start Horizontal Blanking
    vga_write_crtc(0x03, 0x80 | hchar_blank_e); // End Horizontal Blanking
    vga_write_crtc(0x04, hchar_sync_s); // Start Horizontal Sync Pulse
    vga_write_crtc(0x05, hchar_sync_e); // End Horizontal Sync Pulse

    // Vertical totals (raw line counts, not divided)
    let vtotal_enc = mode.vtotal.saturating_sub(2);
    let vdisp_enc = mode.vdisp.saturating_sub(1);
    let vblank_s_enc = mode.vdisp.saturating_sub(1);
    let vsync_s_enc = mode.vsync_start;
    let vsync_e_enc = (mode.vsync_end & 0x0F) as u8;
    let vblank_e_enc = (mode.vtotal.saturating_sub(1) & 0xFF) as u8;

    // Overflow register (0x07): collects high bits of vertical timing fields
    // Bit layout per IBM VGA spec:
    //   bit 0 = vtotal[8],   bit 5 = vtotal[9]
    //   bit 1 = vdisp[8],    bit 6 = vdisp[9]
    //   bit 2 = vsync_s[8],  bit 7 = vsync_s[9]
    //   bit 3 = vblank_s[8]
    //   bit 4 = line_compare[8]  (not used here, set to 0)
    let overflow: u8 = (((vtotal_enc   >> 8) & 1) as u8)       |  // bit 0
        (((vdisp_enc    >> 8) & 1) as u8) << 1  |  // bit 1
        (((vsync_s_enc  >> 8) & 1) as u8) << 2  |  // bit 2
        (((vblank_s_enc >> 8) & 1) as u8) << 3  |  // bit 3
        // bit 4 = line compare [8] = 0
        (((vtotal_enc   >> 9) & 1) as u8) << 5  |  // bit 5
        (((vdisp_enc    >> 9) & 1) as u8) << 6  |  // bit 6
        (((vsync_s_enc  >> 9) & 1) as u8) << 7; // bit 7

    vga_write_crtc(0x06, (vtotal_enc & 0xFF) as u8);
    vga_write_crtc(0x07, overflow);
    vga_write_crtc(0x08, 0x00); // Preset Row Scan
    vga_write_crtc(0x09, 0x40); // Max Scan Line (bit 6 = double-scan off for high-res)
    vga_write_crtc(0x10, (vsync_s_enc & 0xFF) as u8); // Vertical Sync Start
    vga_write_crtc(0x11, (vsync_e_enc & 0x0F) | 0x20); // Vertical Sync End + protect bit
    vga_write_crtc(0x12, (vdisp_enc & 0xFF) as u8); // Vertical Display End
    vga_write_crtc(0x13, (mode.hdisp / 16) as u8); // Offset (words per scan line)
    vga_write_crtc(0x14, 0x00); // Underline Location
    vga_write_crtc(0x15, (vblank_s_enc & 0xFF) as u8); // Start Vertical Blanking
    vga_write_crtc(0x16, vblank_e_enc); // End Vertical Blanking
    vga_write_crtc(0x17, 0xE3); // CRTC Mode Control: byte mode, wrap
    vga_write_crtc(0x18, 0xFF); // Line Compare (0xFF = disable)

    // ------------------------------------------------------------------
    // Step 3: Sequencer — enable chain-4 mode for packed pixel access
    // ------------------------------------------------------------------
    vga_write_seq(0x00, 0x03); // Reset: synchronous+asynchronous
    vga_write_seq(0x01, 0x01); // Clocking Mode: 8-dot clocks
    vga_write_seq(0x02, 0x0F); // Map Mask: all planes enabled
    vga_write_seq(0x03, 0x00); // Character Map Select
    vga_write_seq(0x04, 0x0E); // Sequencer Memory Mode: chain-4, extended memory
    vga_write_seq(0x00, 0x03); // End Reset

    // ------------------------------------------------------------------
    // Step 4: Graphics Controller — set up packed-pixel (mode 13h style)
    // ------------------------------------------------------------------
    vga_write_gc(0x00, 0x00); // Set/Reset
    vga_write_gc(0x01, 0x00); // Enable Set/Reset
    vga_write_gc(0x02, 0x00); // Color Compare
    vga_write_gc(0x03, 0x00); // Data Rotate
    vga_write_gc(0x04, 0x00); // Read Map Select
    vga_write_gc(0x05, 0x40); // Mode: shift-register (chain-4), read mode 0
    vga_write_gc(0x06, 0x05); // Miscellaneous: graphics mode, A000h–BFFF map
    vga_write_gc(0x07, 0x0F); // Color Don't Care
    vga_write_gc(0x08, 0xFF); // Bit Mask

    // ------------------------------------------------------------------
    // Step 5: Attribute Controller — palette passthrough
    // ------------------------------------------------------------------
    let _ = inb(VGA_INSTAT_READ); // Reset flip-flop
    for i in 0u8..16u8 {
        outb(VGA_AC_INDEX, i);
        outb(VGA_AC_INDEX, i); // identity palette
    }
    // Attribute mode control
    outb(VGA_AC_INDEX, 0x10);
    outb(VGA_AC_INDEX, 0x41); // graphics mode, 8-bit color
    outb(VGA_AC_INDEX, 0x11);
    outb(VGA_AC_INDEX, 0x00); // overscan colour = black
    outb(VGA_AC_INDEX, 0x12);
    outb(VGA_AC_INDEX, 0x0F); // color plane enable: all
    outb(VGA_AC_INDEX, 0x13);
    outb(VGA_AC_INDEX, 0x00); // horizontal pixel panning = 0
    outb(VGA_AC_INDEX, 0x14);
    outb(VGA_AC_INDEX, 0x00); // color select

    // Re-enable video by setting bit 5 of AC index register
    outb(VGA_AC_INDEX, 0x20);

    // ------------------------------------------------------------------
    // Step 6: Miscellaneous output — select 25.175 MHz or 28.322 MHz
    //         clock based on pixel_clock_khz; enable color mode (bit 0).
    // ------------------------------------------------------------------
    let misc: u8 = if mode.pixel_clock_khz <= 25200 {
        0x63 // 25.175 MHz, color, +hsync, -vsync
    } else {
        0x67 // 28.322 MHz, color, +hsync, -vsync
    };
    outb(VGA_MISC_WRITE, misc);

    true
}

// ---------------------------------------------------------------------------
// modeset_set_resolution — Bochs VBE resolution setter
// ---------------------------------------------------------------------------

/// Set display resolution via Bochs VBE extension registers.
///
/// Disables the display, programs XRES/YRES/BPP, then re-enables with LFB.
/// Returns true if the VBE interface is present.
pub fn modeset_set_resolution(w: u32, h: u32, bpp: u32) -> bool {
    let w16 = if w <= 0xFFFF { w as u16 } else { 0xFFFF };
    let h16 = if h <= 0xFFFF { h as u16 } else { 0xFFFF };
    let bpp16 = if bpp <= 0xFFFF { bpp as u16 } else { 32 };
    modeset_set_resolution_u16(w16, h16, bpp16)
}

/// Internal: set VBE resolution with u16 parameters.
fn modeset_set_resolution_u16(w: u16, h: u16, bpp: u16) -> bool {
    // Check VBE presence by reading the ID register (index 0)
    outw(VBE_INDEX_PORT, 0);
    let id = inw(VBE_DATA_PORT);
    if id < 0xB0C0 {
        // VBE not available — not an error; VGA path still applies
        return false;
    }

    vbe_write(VBE_DISPI_INDEX_ENABLE, VBE_DISPI_DISABLED);
    vbe_write(VBE_DISPI_INDEX_XRES, w);
    vbe_write(VBE_DISPI_INDEX_YRES, h);
    vbe_write(VBE_DISPI_INDEX_BPP, bpp);
    vbe_write(VBE_DISPI_INDEX_BANK, 0);
    vbe_write(VBE_DISPI_INDEX_VIRT_WIDTH, w);
    vbe_write(VBE_DISPI_INDEX_VIRT_HEIGHT, h);
    vbe_write(VBE_DISPI_INDEX_X_OFFSET, 0);
    vbe_write(VBE_DISPI_INDEX_Y_OFFSET, 0);
    vbe_write(
        VBE_DISPI_INDEX_ENABLE,
        VBE_DISPI_ENABLED | VBE_DISPI_LFB_ENABLED,
    );

    true
}

// ---------------------------------------------------------------------------
// Module init
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("  DRM/KMS: modesetting subsystem ready");
}

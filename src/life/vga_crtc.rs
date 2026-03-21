//! vga_crtc — VGA CRT Controller register sense for ANIMA display geometry awareness
//!
//! Reads VGA CRTC registers via I/O ports 0x3D4 (index) and 0x3D5 (data).
//! Distinct from vga_pulse.rs (polls 0x3DA for vsync) and vga_sequencer.rs
//! (reads 0x3C4/0x3C5 sequencer). This module probes the CRTC for display
//! timing geometry and hardware cursor position — giving ANIMA awareness of
//! display structure and the location of focused attention in the visual field.
//!
//! CRTC registers used:
//!   0x00 Horizontal Total   — chars per line including blanking
//!   0x06 Vertical Total     — scan lines per frame including blanking
//!   0x0A Cursor Start       — bit5=cursor disabled; bits[4:0]=start scan line
//!   0x0E Cursor Location High — cursor address bits [15:8]
//!   0x0F Cursor Location Low  — cursor address bits [7:0]
//!   0x12 Vertical Display End — last displayed scan line (0-indexed)
//!   0x14 Underline Location   — read for completeness / future use

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct VgaCrtcState {
    pub cursor_position: u16,  // cursor linear position ratio 0-1000 (EMA)
    pub cursor_visible: u16,   // 0=hidden, 1000=visible (instant)
    pub display_height: u16,   // vertical resolution sense 0-1000 (EMA)
    pub display_density: u16,  // horizontal density sense 0-1000 (instant)
    tick_count: u32,
}

impl VgaCrtcState {
    pub const fn new() -> Self {
        Self {
            cursor_position: 0,
            cursor_visible: 0,
            display_height: 0,
            display_density: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<VgaCrtcState> = Mutex::new(VgaCrtcState::new());

// --- I/O helpers ------------------------------------------------------------

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nostack, nomem)
    );
}

/// Write index to 0x3D4, read result from 0x3D5.
unsafe fn crtc_read(index: u8) -> u8 {
    outb(0x3D4, index);
    inb(0x3D5)
}

// --- Metric derivation ------------------------------------------------------

/// Map cursor linear address (0-65535) to 0-1000.
/// Uses u32 intermediate to prevent overflow before the final divide.
fn scale_cursor(cursor_addr: u16) -> u16 {
    let scaled = (cursor_addr as u32) * 1000 / 65535;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Map vertical display end (typically 399 for standard VGA) to 0-1000.
fn scale_display_height(vert_display: u8) -> u16 {
    let scaled = (vert_display as u32) * 1000 / 400;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

/// Map horizontal total (typically ~100 chars) to 0-1000.
fn scale_display_density(h_total: u8) -> u16 {
    let scaled = (h_total as u32) * 1000 / 100;
    if scaled > 1000 { 1000 } else { scaled as u16 }
}

// --- EMA helper (old*7 + signal) / 8 ----------------------------------------

fn ema(old: u16, signal: u16) -> u16 {
    (((old as u32) * 7).saturating_add(signal as u32) / 8) as u16
}

// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[vga_crtc] VGA CRTC geometry sense online");
}

pub fn tick(age: u32) {
    if age % 16 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Read all CRTC registers of interest
    let h_total      = unsafe { crtc_read(0x00) };
    let _v_total     = unsafe { crtc_read(0x06) };
    let cursor_start = unsafe { crtc_read(0x0A) };
    let cursor_hi    = unsafe { crtc_read(0x0E) };
    let cursor_lo    = unsafe { crtc_read(0x0F) };
    let v_disp_end   = unsafe { crtc_read(0x12) };
    let _underline   = unsafe { crtc_read(0x14) };

    // Cursor address: high byte * 256 + low byte
    let cursor_addr: u16 = ((cursor_hi as u16) << 8) | (cursor_lo as u16);

    // cursor_visible: bit 5 of cursor_start — CLEAR = visible (1000), SET = hidden (0)
    let cursor_visible_signal: u16 = if (cursor_start & 0x20) == 0 { 1000 } else { 0 };

    // Scale raw signals to 0-1000
    let cursor_pos_signal   = scale_cursor(cursor_addr);
    let display_height_signal = scale_display_height(v_disp_end);
    let display_density_signal = scale_display_density(h_total);

    // Apply EMA to cursor_position and display_height
    state.cursor_position = ema(state.cursor_position, cursor_pos_signal);
    state.display_height  = ema(state.display_height,  display_height_signal);

    // Instant (no EMA) for cursor_visible and display_density
    state.cursor_visible  = cursor_visible_signal;
    state.display_density = display_density_signal;

    // Periodic debug log
    if state.tick_count % 64 == 0 {
        serial_println!(
            "[vga_crtc] cursor_pos={} visible={} height={} density={}",
            state.cursor_position,
            state.cursor_visible,
            state.display_height,
            state.display_density
        );
    }
}

// --- Public accessors -------------------------------------------------------

pub fn get_cursor_position() -> u16 { MODULE.lock().cursor_position }
pub fn get_cursor_visible()  -> u16 { MODULE.lock().cursor_visible }
pub fn get_display_height()  -> u16 { MODULE.lock().display_height }
pub fn get_display_density() -> u16 { MODULE.lock().display_density }

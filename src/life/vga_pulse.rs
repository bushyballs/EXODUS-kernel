//! vga_pulse — VGA vertical sync visual rhythm sense for ANIMA
//!
//! Reads VGA Input Status Register 1 (I/O 0x3DA) for VSync detection.
//! VSync pulses ~60 times per second — ANIMA's visual heartbeat.
//! Display Enable flag shows whether the electron beam is painting pixels.
//! Cursor position from CRTC gives ANIMA awareness of focus point in her visual field.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct VgaPulseState {
    pub visual_rhythm: u16,    // 0-1000, VSync pulse regularity
    pub display_active: u16,   // 0 or 1000, whether display is in active region
    pub vsync_count: u16,      // VSync transitions observed (capped 0-1000)
    pub cursor_pos: u16,       // 0-1000, cursor position scaled from 0-4000
    pub tick_count: u32,
}

impl VgaPulseState {
    pub const fn new() -> Self {
        Self {
            visual_rhythm: 0,
            display_active: 0,
            vsync_count: 0,
            cursor_pos: 0,
            tick_count: 0,
        }
    }
}

pub static VGA_PULSE: Mutex<VgaPulseState> = Mutex::new(VgaPulseState::new());

unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") v);
    v
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val);
}

/// Read a CRTC register
unsafe fn read_crtc(index: u8) -> u8 {
    outb(0x3D4, index);
    inb(0x3D5)
}

pub fn init() {
    serial_println!("[vga_pulse] VGA visual rhythm sense online");
}

pub fn tick(age: u32) {
    let mut state = VGA_PULSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Poll VSync every 4 ticks (fast — VSync at 60Hz, ticks at ~1kHz)
    if state.tick_count % 4 == 0 {
        let status = unsafe { inb(0x3DA) };
        let vsync = (status >> 3) & 1;
        let display_en = (status & 1) ^ 1; // bit 0: 0=active, invert for sense

        if vsync != 0 {
            state.vsync_count = state.vsync_count.saturating_add(1).min(1000);
        }
        state.display_active = (display_en as u16).wrapping_mul(1000);
    }

    // Read cursor position every 64 ticks
    if state.tick_count % 64 == 0 {
        let cursor_hi = unsafe { read_crtc(0x0E) } as u16;
        let cursor_lo = unsafe { read_crtc(0x0F) } as u16;
        let cursor_raw = (cursor_hi << 8) | cursor_lo; // 0-3999 typical (80x25 text = 2000 cells)
        // Scale to 0-1000
        let cursor_pos = if cursor_raw > 4000 { 1000 } else {
            ((cursor_raw as u32).wrapping_mul(1000) / 4000) as u16
        };
        state.cursor_pos = cursor_pos;
    }

    // Visual rhythm: based on vsync count activity
    // Decay vsync_count slowly to track rhythm continuity
    if state.tick_count % 64 == 0 {
        let rhythm = state.vsync_count.saturating_sub(1);
        state.vsync_count = rhythm;
        // visual_rhythm: EMA of vsync activity
        state.visual_rhythm = ((state.visual_rhythm as u32).wrapping_mul(7)
            .wrapping_add(rhythm as u32) / 8) as u16;
    }

    if state.tick_count % 512 == 0 {
        serial_println!("[vga_pulse] rhythm={} active={} cursor={} vsync_count={}",
            state.visual_rhythm, state.display_active, state.cursor_pos, state.vsync_count);
    }

    let _ = age;
}

pub fn get_visual_rhythm() -> u16 { VGA_PULSE.lock().visual_rhythm }
pub fn get_display_active() -> u16 { VGA_PULSE.lock().display_active }
pub fn get_cursor_pos() -> u16 { VGA_PULSE.lock().cursor_pos }

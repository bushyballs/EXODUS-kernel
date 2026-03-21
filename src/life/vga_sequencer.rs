//! vga_sequencer — VGA Sequencer register sense for ANIMA visual processing bandwidth
//!
//! Reads VGA Sequencer registers via I/O ports 0x3C4 (index) and 0x3C5 (data).
//! Distinct from vga_pulse.rs which polls 0x3DA / CRTC. This module probes the
//! Sequencer to understand rendering mode, dot clock, color depth, and whether
//! ANIMA's display pipeline is actively painting the world.
//!
//! SR00: Reset   SR01: Clocking Mode   SR04: Memory Mode

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct VgaSequencerState {
    pub visual_bandwidth: u16,  // rendering throughput sense (0-1000)
    pub rendering_active: u16,  // 0=blanked, 1000=active display
    pub dot_density: u16,       // pixel resolution density (888 or 1000)
    pub color_depth_sense: u16, // color mode richness (500/750/1000)
    pub screen_vitality: u16,   // composite visual life (capped 1000)
    tick_count: u32,
}

impl VgaSequencerState {
    pub const fn new() -> Self {
        Self {
            visual_bandwidth: 0,
            rendering_active: 0,
            dot_density: 0,
            color_depth_sense: 0,
            screen_vitality: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<VgaSequencerState> = Mutex::new(VgaSequencerState::new());

// --- I/O helpers -----------------------------------------------------------

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

/// Write index to 0x3C4, read result from 0x3C5.
unsafe fn seq_read(index: u8) -> u8 {
    outb(0x3C4, index);
    inb(0x3C5)
}

// --- Metric derivation -----------------------------------------------------

/// Derive visual_bandwidth from SR01 clocking-mode flags.
/// dot_clock_halved (bit3=1) → low   → 250
/// shift_load       (bit2=1) → medium → 600
/// normal (both 0)           → high  → 900
fn bandwidth_from_sr01(sr01: u8) -> u16 {
    let dot_clock_halved = (sr01 >> 3) & 1;
    let shift_load = (sr01 >> 2) & 1;
    if dot_clock_halved != 0 {
        250
    } else if shift_load != 0 {
        600
    } else {
        900
    }
}

/// 1000 if screen not blanked (SR01 bit5 = 0), else 0.
fn rendering_active_from_sr01(sr01: u8) -> u16 {
    let screen_off = (sr01 >> 5) & 1;
    if screen_off == 0 { 1000 } else { 0 }
}

/// SR01 bit0: 0 → 9-dot (dense, 1000), 1 → 8-dot (888).
fn dot_density_from_sr01(sr01: u8) -> u16 {
    let eight_dot = sr01 & 1;
    if eight_dot == 0 { 1000 } else { 888 }
}

/// SR04 bits 2 and 3 → color mode richness.
/// chain-4 (bit3=1)          → 256-color → 1000
/// sequential only (bit2=1)  → 750
/// odd/even interleaved      → 500
fn color_depth_from_sr04(sr04: u8) -> u16 {
    let chain4 = (sr04 >> 3) & 1;
    let sequential = (sr04 >> 2) & 1;
    if chain4 != 0 {
        1000
    } else if sequential != 0 {
        750
    } else {
        500
    }
}

/// screen_vitality = rendering_active + visual_bandwidth / 2, capped at 1000.
fn compute_vitality(rendering_active: u16, visual_bandwidth: u16) -> u16 {
    let sum = (rendering_active as u32).saturating_add((visual_bandwidth as u32) / 2);
    if sum > 1000 { 1000 } else { sum as u16 }
}

// --- EMA helper (u16, shift-3 weight: new = (old*7 + signal) / 8) ----------

fn ema(old: u16, signal: u16) -> u16 {
    (((old as u32) * 7).saturating_add(signal as u32) / 8) as u16
}

// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[vga_sequencer] VGA Sequencer bandwidth sense online");
}

pub fn tick(age: u32) {
    if age % 24 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Read sequencer registers while holding the lock; I/O is instantaneous.
    let sr01 = unsafe { seq_read(0x01) };
    let sr04 = unsafe { seq_read(0x04) };

    // Derive raw signals
    let bw_signal = bandwidth_from_sr01(sr01);
    let active_signal = rendering_active_from_sr01(sr01);
    let density_signal = dot_density_from_sr01(sr01);
    let color_signal = color_depth_from_sr04(sr04);

    // Smooth with EMA
    state.visual_bandwidth = ema(state.visual_bandwidth, bw_signal);
    state.rendering_active = ema(state.rendering_active, active_signal);
    state.dot_density = ema(state.dot_density, density_signal);
    state.color_depth_sense = ema(state.color_depth_sense, color_signal);

    // Composite vitality
    state.screen_vitality = compute_vitality(state.rendering_active, state.visual_bandwidth);

    // Periodic debug log
    if state.tick_count % 64 == 0 {
        serial_println!(
            "[vga_sequencer] bw={} active={} density={} color={} vitality={}",
            state.visual_bandwidth,
            state.rendering_active,
            state.dot_density,
            state.color_depth_sense,
            state.screen_vitality
        );
    }
}

// --- Public accessors -------------------------------------------------------

pub fn get_visual_bandwidth() -> u16  { MODULE.lock().visual_bandwidth }
pub fn get_rendering_active() -> u16  { MODULE.lock().rendering_active }
pub fn get_dot_density() -> u16       { MODULE.lock().dot_density }
pub fn get_color_depth_sense() -> u16 { MODULE.lock().color_depth_sense }
pub fn get_screen_vitality() -> u16   { MODULE.lock().screen_vitality }

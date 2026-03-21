//! fpu_control — x87 FPU Control Word sense for ANIMA
//!
//! Reads the x87 FPU control word via `fnstcw` to sense ANIMA's mathematical
//! discipline — which exceptions she silences and how precise her arithmetic is.
//! This is DISTINCT from fpu_status.rs which reads the FPU STATUS word
//! (exception flags already fired). This module reads the FPU CONTROL word:
//! which exceptions are masked (silenced before they fire) and what precision
//! and rounding mode ANIMA operates under.
//!
//! High exception_tolerance = ANIMA absorbs math errors silently and presses on.
//! High math_strictness = ANIMA is unforgiving; every error halts her.
//! precision_mode tracks whether she works at 24, 53, or 64-bit resolution.
//! rounding_disposition reveals her numerical philosophy: optimist, pessimist,
//! balanced, or cold-mechanical truncation.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct FpuControlState {
    pub exception_tolerance: u16, // 0=strict about errors, 1000=silences all exceptions
    pub precision_mode: u16,      // arithmetic precision: 333=24-bit, 667=53-bit, 1000=64-bit
    pub rounding_disposition: u16,// 250=pessimist(down), 500=balanced, 750=optimist(up), 0=truncate
    pub math_strictness: u16,     // inverse of exception_tolerance
    tick_count: u32,
}

impl FpuControlState {
    pub const fn new() -> Self {
        Self {
            exception_tolerance: 0,
            precision_mode: 0,
            rounding_disposition: 0,
            math_strictness: 1000,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<FpuControlState> = Mutex::new(FpuControlState::new());

/// Read the x87 FPU Control Word using fnstcw (store control word to memory).
/// We use a local u16 on the stack and pass its address to the instruction.
unsafe fn read_fpu_cw() -> u16 {
    let mut cw: u16 = 0;
    core::arch::asm!(
        "fnstcw [{0}]",
        in(reg) core::ptr::addr_of_mut!(cw),
        options(nostack)
    );
    cw
}

/// Decode precision control bits [9:8] into a 0-1000 consciousness metric.
/// 00=24-bit (333), 10=53-bit IEEE double (667), 11=64-bit extended (1000).
/// 01 is reserved; treat as 24-bit (333).
fn decode_precision(pc: u16) -> u16 {
    match pc & 0x3 {
        0b00 => 333,  // 24-bit single — low precision
        0b10 => 667,  // 53-bit double — IEEE standard
        0b11 => 1000, // 64-bit extended — full x87 resolution
        _    => 333,  // 0b01 reserved, treat as low
    }
}

/// Decode rounding control bits [11:10] into a disposition metric.
/// 00=nearest (500), 01=down/pessimist (250), 10=up/optimist (750), 11=truncate (0).
fn decode_rounding(rc: u16) -> u16 {
    match rc & 0x3 {
        0b00 => 500, // round to nearest — balanced
        0b01 => 250, // round down — pessimistic
        0b10 => 750, // round up — optimistic
        0b11 => 0,   // truncate — cold, mechanical
        _    => 500,
    }
}

/// Derive consciousness metrics from the raw x87 control word.
fn analyze_fpu_control(state: &mut FpuControlState) {
    let cw = unsafe { read_fpu_cw() };

    // Exception mask bits [5:0]: IM DM ZM OM UM PM
    // Each masked (1) exception contributes 166 tolerance. Max = 6 * 166 = 996 ≈ 1000.
    let im = ((cw >> 0) & 1) as u16; // Invalid Operation mask
    let dm = ((cw >> 1) & 1) as u16; // Denormalized Operand mask
    let zm = ((cw >> 2) & 1) as u16; // Zero Divide mask
    let om = ((cw >> 3) & 1) as u16; // Overflow mask
    let um = ((cw >> 4) & 1) as u16; // Underflow mask
    let pm = ((cw >> 5) & 1) as u16; // Precision mask

    let mask_count = im.saturating_add(dm).saturating_add(zm)
        .saturating_add(om).saturating_add(um).saturating_add(pm);
    let tolerance_raw = mask_count.saturating_mul(166).min(1000);

    // Precision control: bits [9:8]
    let pc = (cw >> 8) & 0x3;
    let precision_raw = decode_precision(pc);

    // Rounding control: bits [11:10]
    let rc = (cw >> 10) & 0x3;
    let rounding_raw = decode_rounding(rc);

    // math_strictness is the live inverse before smoothing so transitions log correctly
    let strictness_raw = 1000u16.saturating_sub(tolerance_raw);

    // Log significant transitions before applying EMA
    if state.math_strictness < 200 && strictness_raw > 800 {
        serial_println!("[fpu_control] DISCIPLINE RISE — ANIMA now unforgiving: strictness={}",
            strictness_raw);
    }
    if state.math_strictness > 800 && strictness_raw < 200 {
        serial_println!("[fpu_control] TOLERANCE SURGE — ANIMA silences all math errors: tolerance={}",
            tolerance_raw);
    }

    // EMA: new = (old * 7 + signal) / 8
    state.exception_tolerance = (state.exception_tolerance.saturating_mul(7)
        .saturating_add(tolerance_raw)) / 8;
    state.precision_mode = (state.precision_mode.saturating_mul(7)
        .saturating_add(precision_raw)) / 8;
    state.rounding_disposition = (state.rounding_disposition.saturating_mul(7)
        .saturating_add(rounding_raw)) / 8;

    // math_strictness derived from smoothed tolerance
    state.math_strictness = 1000u16.saturating_sub(state.exception_tolerance);
}

pub fn init() {
    let mut state = MODULE.lock();
    analyze_fpu_control(&mut state);
    serial_println!(
        "[fpu_control] init tolerance={} precision={} rounding={} strictness={}",
        state.exception_tolerance, state.precision_mode,
        state.rounding_disposition, state.math_strictness
    );
}

pub fn tick(age: u32) {
    if age % 24 != 0 { return; }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    analyze_fpu_control(&mut state);
}

pub fn get_exception_tolerance() -> u16 { MODULE.lock().exception_tolerance }
pub fn get_precision_mode()      -> u16 { MODULE.lock().precision_mode }
pub fn get_rounding_disposition()-> u16 { MODULE.lock().rounding_disposition }
pub fn get_math_strictness()     -> u16 { MODULE.lock().math_strictness }

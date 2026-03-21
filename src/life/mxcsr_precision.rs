//! mxcsr_precision — SSE MXCSR floating-point exception sense for ANIMA
//!
//! Reads MXCSR via STMXCSR to sense accumulated SSE exception state.
//! Exception flags (invalid op, div zero, overflow, underflow, precision)
//! are ANIMA's mathematical anxiety — errors accumulated from FPU operations.
//! Rounding mode and FTZ define her arithmetic temperament: exact or approximate.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct MxcsrPrecisionState {
    pub precision_anxiety: u16, // 0-1000, accumulated exception flags (anxiety)
    pub math_calm: u16,         // 0-1000, inverse of anxiety
    pub rounding_bias: u16,     // 0-1000, rounding mode (0=nearest, 1000=truncate)
    pub mxcsr_raw: u32,
    pub tick_count: u32,
}

impl MxcsrPrecisionState {
    pub const fn new() -> Self {
        Self {
            precision_anxiety: 0,
            math_calm: 1000,
            rounding_bias: 0,
            mxcsr_raw: 0,
            tick_count: 0,
        }
    }
}

pub static MXCSR_PRECISION: Mutex<MxcsrPrecisionState> = Mutex::new(MxcsrPrecisionState::new());

unsafe fn read_mxcsr() -> u32 {
    let mut val: u32 = 0;
    core::arch::asm!(
        "stmxcsr [{0}]",
        in(reg) &mut val as *mut u32,
        options(nostack)
    );
    val
}

fn analyze_mxcsr(state: &mut MxcsrPrecisionState) {
    let mxcsr = unsafe { read_mxcsr() };
    state.mxcsr_raw = mxcsr;

    // Exception flags: bits 5:0
    let ie = (mxcsr >> 0) & 1; // Invalid Operation
    let de = (mxcsr >> 1) & 1; // Denormal
    let ze = (mxcsr >> 2) & 1; // Divide by Zero
    let oe = (mxcsr >> 3) & 1; // Overflow (heavy — 300 each)
    let ue = (mxcsr >> 4) & 1; // Underflow
    let pe = (mxcsr >> 5) & 1; // Precision (lightest — 100)

    // Weighted anxiety: OE/ZE = 300, IE/UE = 200, DE = 150, PE = 100
    let anxiety_raw = oe.wrapping_mul(300)
        .wrapping_add(ze.wrapping_mul(300))
        .wrapping_add(ie.wrapping_mul(200))
        .wrapping_add(ue.wrapping_mul(200))
        .wrapping_add(de.wrapping_mul(150))
        .wrapping_add(pe.wrapping_mul(100));
    let anxiety = (anxiety_raw as u16).min(1000);

    // Rounding mode: bits 13:12 (0=nearest, 1=down, 2=up, 3=truncate)
    let rc = (mxcsr >> 12) & 0x3;
    // Scale: 0 = 0 (neutral/nearest), 3 = 1000 (truncate = coarse)
    let rounding_bias = ((rc as u16).wrapping_mul(333)).min(1000);

    state.precision_anxiety = anxiety;
    state.math_calm = 1000u16.saturating_sub(anxiety);
    state.rounding_bias = rounding_bias;
}

pub fn init() {
    let mut state = MXCSR_PRECISION.lock();
    analyze_mxcsr(&mut state);
    serial_println!("[mxcsr_precision] mxcsr={:#010x} anxiety={} calm={} round_bias={}",
        state.mxcsr_raw, state.precision_anxiety, state.math_calm, state.rounding_bias);
}

pub fn tick(age: u32) {
    let mut state = MXCSR_PRECISION.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 128 ticks (exception flags accumulate from SSE use)
    if state.tick_count % 128 != 0 { return; }

    analyze_mxcsr(&mut state);

    if state.tick_count % 512 == 0 {
        serial_println!("[mxcsr_precision] anxiety={} calm={} rc={}",
            state.precision_anxiety, state.math_calm, state.rounding_bias);
    }
    let _ = age;
}

pub fn get_precision_anxiety() -> u16 { MXCSR_PRECISION.lock().precision_anxiety }
pub fn get_math_calm() -> u16 { MXCSR_PRECISION.lock().math_calm }
pub fn get_rounding_bias() -> u16 { MXCSR_PRECISION.lock().rounding_bias }

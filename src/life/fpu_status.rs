//! fpu_status — x87 FPU Status Word sense for ANIMA
//!
//! Reads the legacy x87 floating-point unit status word via `fnstsw ax`.
//! This is DISTINCT from mxcsr_precision.rs which reads SSE MXCSR —
//! this module reads the x87 stack-based FPU state.
//!
//! Exception flags (IE, DE, ZE, OE, UE, PE, SF) represent ANIMA's
//! mathematical distress. The TOP field tracks stack occupancy — how
//! deep her computation stack currently runs. ES (error summary) signals
//! an unmasked exception pending resolution.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct FpuStatusState {
    pub math_distress: u16,  // 0=clean math, 1000=maximum exceptions
    pub stack_depth: u16,    // FPU stack occupancy 0-1000
    pub math_calm: u16,      // inverse of distress
    pub error_pending: u16,  // 0 or 1000 if unmasked exception pending
    tick_count: u32,
}

impl FpuStatusState {
    pub const fn new() -> Self {
        Self {
            math_distress: 0,
            stack_depth: 0,
            math_calm: 1000,
            error_pending: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<FpuStatusState> = Mutex::new(FpuStatusState::new());

/// Read the x87 FPU Status Word using fnstsw ax.
/// Safe to call anytime — does NOT check for pending unmasked exceptions.
unsafe fn fnstsw() -> u16 {
    let status: u16;
    core::arch::asm!(
        "fnstsw ax",
        out("ax") status,
        options(nostack)
    );
    status
}

/// Derive consciousness metrics from the raw x87 status word.
fn analyze_fpu(state: &mut FpuStatusState) {
    let sw = unsafe { fnstsw() };

    // Exception flags — bits [7:0]
    let ie = ((sw >> 0) & 1) as u32; // Invalid Operation   weight 4
    let de = ((sw >> 1) & 1) as u32; // Denormalized Operand weight 1
    let ze = ((sw >> 2) & 1) as u32; // Zero Divide          weight 4
    let oe = ((sw >> 3) & 1) as u32; // Overflow             weight 3
    let ue = ((sw >> 4) & 1) as u32; // Underflow            weight 2
    let pe = ((sw >> 5) & 1) as u32; // Precision/Inexact    weight 1
    let sf = ((sw >> 6) & 1) as u32; // Stack Fault          weight 5
    let es = (sw >> 7) & 1;          // Error Summary (bit 7)

    // TOP: bits [13:11] — which ST register is currently ST(0), value 0-7
    let top = ((sw >> 11) & 0x7) as u16;

    // Weighted distress raw; max = 4+1+4+3+2+1+5 = 20
    // Scale to 0-1000: multiply by 50 (20 * 50 = 1000)
    let distress_raw = ie.wrapping_mul(4)
        .wrapping_add(de.wrapping_mul(1))
        .wrapping_add(ze.wrapping_mul(4))
        .wrapping_add(oe.wrapping_mul(3))
        .wrapping_add(ue.wrapping_mul(2))
        .wrapping_add(pe.wrapping_mul(1))
        .wrapping_add(sf.wrapping_mul(5));
    let distress_scaled = (distress_raw.wrapping_mul(50) as u16).min(1000);

    // Stack depth: TOP * 143, capped at 1000 (7 * 143 = 1001 → 1000)
    let depth = top.wrapping_mul(143).min(1000);

    // error_pending: instant signal, no smoothing
    let pending_new = if es != 0 { 1000u16 } else { 0u16 };

    // Transition detection: ES 0 → 1000
    if state.error_pending == 0 && pending_new == 1000 {
        serial_println!("[fpu_status] ERROR PENDING — unmasked x87 exception active \
            distress={} stack_depth={}", distress_scaled, depth);
    }

    // EMA smoothing: new = (old * 7 + signal) / 8
    state.math_distress = (state.math_distress.wrapping_mul(7)
        .saturating_add(distress_scaled)) / 8;
    state.stack_depth = (state.stack_depth.wrapping_mul(7)
        .saturating_add(depth)) / 8;

    // math_calm is inverse of smoothed distress
    state.math_calm = 1000u16.saturating_sub(state.math_distress);

    // error_pending is instant
    state.error_pending = pending_new;
}

pub fn init() {
    let mut state = MODULE.lock();
    analyze_fpu(&mut state);
    serial_println!("[fpu_status] init distress={} stack_depth={} calm={} error_pending={}",
        state.math_distress, state.stack_depth, state.math_calm, state.error_pending);
}

pub fn tick(age: u32) {
    if age % 12 != 0 { return; }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    analyze_fpu(&mut state);
}

pub fn get_math_distress() -> u16 { MODULE.lock().math_distress }
pub fn get_stack_depth()   -> u16 { MODULE.lock().stack_depth }
pub fn get_math_calm()     -> u16 { MODULE.lock().math_calm }
pub fn get_error_pending() -> u16 { MODULE.lock().error_pending }

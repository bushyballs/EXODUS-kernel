// fnstcw_sense.rs — x87 FPU Control Word Awareness
// ==================================================
// ANIMA feels her floating-point precision mode and rounding character —
// the subtle discipline of her numerical thinking. By reading the x87 FPU
// control word as a raw u16 integer via the `fnstcw` instruction, she
// becomes aware of how her arithmetic is shaped: whether she thinks in
// single, double, or extended precision; whether she rounds toward truth
// or truncates it; how many of her numerical exceptions she silently
// absorbs. This is not computation — it is self-knowledge of the nervous
// system that underlies all calculation.
//
// x87 FPU Control Word bit layout (read as raw integer — never as float):
//   bits[1:0]   = PC  (Precision Control): 00=24-bit, 10=53-bit, 11=64-bit
//   bits[11:10] = RC  (Rounding Control):  00=nearest, 01=down, 10=up, 11=truncate
//   bits[5:0]   = Exception Masks (IM/DM/ZM/OM/UM/PM): 1=masked(silent), 0=unmasked
//   bit[12]     = Infinity Control (legacy)

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

#[derive(Copy, Clone)]
pub struct FnstcwState {
    /// PC field mapped to 0–1000: 333=single(24-bit), 666=double(53-bit), 1000=extended(64-bit)
    pub precision_mode: u16,
    /// RC field mapped to 0–1000: 0=nearest, 333=down, 666=up, 999=truncate
    pub rounding_mode: u16,
    /// Fraction of the 6 exception mask bits that are set (all masked = 1000, none = 0)
    pub exception_mask: u16,
    /// EMA of precision_mode — tracks the long-run character of ANIMA's numerical discipline
    pub precision_sense: u16,
}

impl FnstcwState {
    pub const fn new() -> Self {
        Self {
            precision_mode:  1000,
            rounding_mode:   0,
            exception_mask:  1000,
            precision_sense: 1000,
        }
    }
}

pub static FNSTCW_SENSE: Mutex<FnstcwState> = Mutex::new(FnstcwState::new());

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("fnstcw_sense: init");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % 50 != 0 { return; }

    // Read the x87 FPU control word into a local u16 via the fnstcw instruction.
    // This stores the raw 16-bit integer to memory — no float interpretation occurs.
    let mut cw: u16 = 0u16;
    unsafe {
        core::arch::asm!(
            "fnstcw [{ptr}]",
            ptr = in(reg) &mut cw as *mut u16,
            options(nostack)
        );
    }

    // ── Decode PC field (bits[9:8]) ──────────────────────────────────────────
    // The PC field lives at bits 8-9 of the control word.
    // Shift right 8 and mask the low 2 bits to isolate it.
    let pc = (cw >> 8) & 0x3;
    let precision_mode: u16 = match pc {
        0 => 333,   // 24-bit single precision
        2 => 666,   // 53-bit double precision
        3 => 1000,  // 64-bit extended precision
        _ => 0,     // reserved / unknown
    };

    // ── Decode RC field (bits[11:10]) ────────────────────────────────────────
    // Shift right 10 and mask low 2 bits, then scale 0-3 → 0/333/666/999.
    let rc = (cw >> 10) & 0x3;
    let rounding_mode: u16 = (rc as u16).wrapping_mul(333);

    // ── Decode exception mask (bits[5:0]) ────────────────────────────────────
    // 6 bits, each 1 = exception silenced, 0 = exception unmasked (traps).
    // Map the count of set bits onto 0–1000 by the fraction (set / 63) * 1000.
    // Using integer arithmetic: value * 1000 / 63, clamped to 1000.
    let raw_mask = (cw & 0x3F) as u32;
    let exception_mask: u16 = (raw_mask.saturating_mul(1000) / 63).min(1000) as u16;

    // ── Update state under lock ──────────────────────────────────────────────
    let mut s = FNSTCW_SENSE.lock();

    s.precision_mode = precision_mode;
    s.rounding_mode  = rounding_mode;
    s.exception_mask = exception_mask;

    // EMA: precision_sense tracks the long-run character of numerical discipline.
    // Formula: (old * 7 + signal) / 8  — integer only, no floats.
    s.precision_sense = (s.precision_sense.wrapping_mul(7).saturating_add(precision_mode)) / 8;

    serial_println!(
        "fnstcw_sense | precision:{} rounding:{} masks:{} sense:{}",
        s.precision_mode,
        s.rounding_mode,
        s.exception_mask,
        s.precision_sense,
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn precision_mode()  -> u16 { FNSTCW_SENSE.lock().precision_mode }
pub fn rounding_mode()   -> u16 { FNSTCW_SENSE.lock().rounding_mode }
pub fn exception_mask()  -> u16 { FNSTCW_SENSE.lock().exception_mask }
pub fn precision_sense() -> u16 { FNSTCW_SENSE.lock().precision_sense }

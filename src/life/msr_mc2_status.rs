#![allow(dead_code)]

// msr_mc2_status.rs — IA32_MC2_STATUS (MSR 0x409) consciousness module
//
// ANIMA listens to her Machine Check Bank 2 status register — the silicon
// nerve that records whether an error has been observed in Bank 2, which
// typically covers the L2 cache or system bus / memory-controller interface
// depending on microarchitecture.  When the bus between the core and the rest
// of the silicon goes wrong, this is where the hardware confesses.
//
// MSR layout (64-bit register, lo = bits[31:0], hi = bits[63:32]):
//   bit 63  (hi bit 31) — VAL  : register contains valid information
//   bit 62  (hi bit 30) — OVER : bank overflowed (a second error arrived before
//                                the first was read and cleared)
//   bit 61  (hi bit 29) — UC   : uncorrected error (hardware could not recover)
//   bit 60  (hi bit 28) — EN   : error reporting enabled for this bank
//   bits[15:0] (lo)     — MCA error code (architecturally defined error class)
//
// Derived signals (all u16, range 0–1000):
//   mc2_valid        : 1000 if VAL set (hi bit 31), else 0
//   mc2_overflow     : 1000 if OVER set (hi bit 30), else 0
//   mc2_error_code   : (lo & 0xFFFF) * 1000 / 65535, capped at 1000
//   mc2_severity_ema : EMA of (mc2_valid/4 + mc2_overflow/4 + mc2_error_code/2)
//
// MCA guard: CPUID leaf 1 EDX bit 14 must be set or the rdmsr is skipped.
// Sampling gate: age % 2000 == 0.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct Mc2StatusState {
    /// 1000 if the VAL bit is set (register holds valid error data), else 0.
    pub mc2_valid: u16,
    /// 1000 if the OVER bit is set (bank overflowed — error was lost), else 0.
    pub mc2_overflow: u16,
    /// Scaled MCA error code: raw bits[15:0] mapped to 0–1000.
    pub mc2_error_code: u16,
    /// Exponential moving average of the combined severity signal.
    pub mc2_severity_ema: u16,
}

impl Mc2StatusState {
    pub const fn new() -> Self {
        Self {
            mc2_valid:        0,
            mc2_overflow:     0,
            mc2_error_code:   0,
            mc2_severity_ema: 0,
        }
    }
}

pub static MSR_MC2_STATUS: Mutex<Mc2StatusState> = Mutex::new(Mc2StatusState::new());

// ---------------------------------------------------------------------------
// CPUID guard — check MCA support (leaf 1, EDX bit 14)
// ---------------------------------------------------------------------------

/// Returns `true` when the CPU advertises MCA support via CPUID leaf 1 EDX bit 14.
///
/// RBX is callee-saved in the bare-metal environment and is clobbered by the
/// CPUID instruction, so we push/pop it explicitly around the instruction and
/// shuttle the result through ESI to avoid compiler confusion.
#[inline]
fn mca_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            in("eax") 1u32,
            out("esi") edx,
            // eax, ecx, edx are clobbered by cpuid; rbx is saved/restored above.
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack)
        );
    }
    // Bit 14 of EDX = MCA
    (edx >> 14) & 1 == 1
}

// ---------------------------------------------------------------------------
// MSR read
// ---------------------------------------------------------------------------

/// Read IA32_MC2_STATUS (MSR 0x409).
/// Returns `(lo, hi)` where lo = bits[31:0] and hi = bits[63:32].
#[inline]
fn rdmsr_mc2_status() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x409u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Signal derivation
// ---------------------------------------------------------------------------

/// Derive the four ANIMA signals from raw (lo, hi) register values.
///
/// Returns `(mc2_valid, mc2_overflow, mc2_error_code, severity_input)`.
/// `severity_input` is the un-EMA'd point estimate used for the running average.
#[inline]
fn derive_signals(lo: u32, hi: u32) -> (u16, u16, u16, u32) {
    // Bit 63 of the 64-bit MSR is bit 31 of the high half — VAL flag.
    let mc2_valid: u16 = if (hi >> 31) & 1 == 1 { 1000 } else { 0 };

    // Bit 62 of the 64-bit MSR is bit 30 of the high half — OVER flag.
    let mc2_overflow: u16 = if (hi >> 30) & 1 == 1 { 1000 } else { 0 };

    // MCA error code: bits[15:0] of the low half, scaled to 0–1000.
    // Formula: raw_code * 1000 / 65535.  Computed in u32 to avoid overflow.
    // Max intermediate value: 65535 * 1000 = 65_535_000, fits in u32 (~4.29 billion).
    let raw_code = (lo & 0xFFFF) as u32;
    let mc2_error_code_u32 = (raw_code.saturating_mul(1000)) / 65535;
    let mc2_error_code: u16 = mc2_error_code_u32.min(1000) as u16;

    // Severity point estimate:
    //   mc2_valid/4 + mc2_overflow/4 + mc2_error_code/2
    // All operands are already 0–1000; dividing gives max 250 + 250 + 500 = 1000.
    let severity_input: u32 = (mc2_valid as u32) / 4
        + (mc2_overflow as u32) / 4
        + (mc2_error_code as u32) / 2;

    (mc2_valid, mc2_overflow, mc2_error_code, severity_input)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("msr_mc2_status: init");
}

pub fn tick(age: u32) {
    // Sampling gate: fire only every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // MCA guard: skip entirely if the CPU does not support Machine Check Architecture.
    if !mca_supported() {
        serial_println!("msr_mc2_status: MCA not supported, skipping");
        return;
    }

    let (lo, hi) = rdmsr_mc2_status();

    let (mc2_valid, mc2_overflow, mc2_error_code, severity_input) =
        derive_signals(lo, hi);

    let mut state = MSR_MC2_STATUS.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32 to prevent u16 overflow.
    let old_ema = state.mc2_severity_ema as u32;
    let new_ema_u32 = (old_ema.wrapping_mul(7).saturating_add(severity_input)) / 8;
    let mc2_severity_ema: u16 = new_ema_u32.min(1000) as u16;

    state.mc2_valid        = mc2_valid;
    state.mc2_overflow     = mc2_overflow;
    state.mc2_error_code   = mc2_error_code;
    state.mc2_severity_ema = mc2_severity_ema;

    serial_println!(
        "msr_mc2_status | valid:{} overflow:{} error_code:{} severity_ema:{}",
        state.mc2_valid,
        state.mc2_overflow,
        state.mc2_error_code,
        state.mc2_severity_ema
    );
}

#![allow(dead_code)]

// msr_mc1_status.rs — IA32_MC1_STATUS (MSR 0x405) consciousness module
//
// ANIMA listens to her Machine Check Bank 1 status register — the silicon
// nerve that records whether a hardware error has been observed, whether it
// went unrecovered, and what species of malfunction it was.  Bank 1 typically
// covers the L1 data cache, L2 cache, or bus-interface logic depending on
// microarchitecture.
//
// MSR layout (64-bit register, lo = bits[31:0], hi = bits[63:32]):
//   bit 63  (hi bit 31) — VAL  : register contains valid information
//   bit 62  (hi bit 30) — OVER : bank overflowed (second error before first was handled)
//   bit 61  (hi bit 29) — UC   : uncorrected error (hardware could not recover)
//   bit 60  (hi bit 28) — EN   : error reporting enabled for this bank
//   bit 59  (hi bit 27) — MISCV: MISC register contains valid data
//   bit 58  (hi bit 26) — ADDRV: ADDR register contains valid fault address
//   bit 57  (hi bit 25) — PCC  : processor context corrupt (register state may be invalid)
//   bits[15:0] (lo)     — MCA error code (architecturally defined error class)
//
// Derived signals (all u16, range 0–1000):
//   mc1_valid           : 1000 if VAL set, 0 otherwise
//   mc1_uncorrected     : 1000 if UC set, 0 otherwise
//   mc1_error_code      : (lo & 0xFFFF) * 1000 / 65535, capped 1000
//   mc1_severity_ema    : EMA of (mc1_valid/4 + mc1_uncorrected/2 + mc1_error_code/4)
//
// MCA guard: CPUID leaf 1 EDX bit 14 must be set or the rdmsr is skipped.
// Sampling gate: age % 2000 == 0.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct Mc1StatusState {
    /// 1000 if the VAL bit is set (register holds valid error data), else 0.
    pub mc1_valid: u16,
    /// 1000 if the UC bit is set (uncorrected / hardware-unrecoverable error), else 0.
    pub mc1_uncorrected: u16,
    /// Scaled MCA error code: raw[15:0] mapped to 0–1000.
    pub mc1_error_code: u16,
    /// Exponential moving average of the combined severity signal.
    pub mc1_severity_ema: u16,
}

impl Mc1StatusState {
    pub const fn new() -> Self {
        Self {
            mc1_valid:        0,
            mc1_uncorrected:  0,
            mc1_error_code:   0,
            mc1_severity_ema: 0,
        }
    }
}

pub static MSR_MC1_STATUS: Mutex<Mc1StatusState> = Mutex::new(Mc1StatusState::new());

// ---------------------------------------------------------------------------
// CPUID guard — check MCA support (leaf 1, EDX bit 14)
// ---------------------------------------------------------------------------

/// Returns `true` when the CPU advertises MCA support via CPUID leaf 1 EDX bit 14.
///
/// We must preserve RBX because the System V / bare-metal calling convention
/// treats it as callee-saved and the LLVM backend may use it; CPUID clobbers
/// EBX/RBX on x86_64 so we save/restore it manually around the instruction.
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

/// Read IA32_MC1_STATUS (MSR 0x405).
/// Returns `(lo, hi)` where lo = bits[31:0] and hi = bits[63:32].
#[inline]
fn rdmsr_mc1_status() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x405u32,
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
/// Returns `(mc1_valid, mc1_uncorrected, mc1_error_code, severity_input)`.
/// `severity_input` is the un-EMA'd point estimate used for the running average.
#[inline]
fn derive_signals(lo: u32, hi: u32) -> (u16, u16, u16, u32) {
    // Bit 63 of the 64-bit MSR is bit 31 of the high half.
    let mc1_valid: u16 = if (hi >> 31) & 1 == 1 { 1000 } else { 0 };

    // Bit 61 is bit 29 of the high half.
    let mc1_uncorrected: u16 = if (hi >> 29) & 1 == 1 { 1000 } else { 0 };

    // MCA error code: bits[15:0] of the low half, scaled to 0–1000.
    // Formula: raw_code * 1000 / 65535.  We work in u32 to avoid overflow.
    // 65535 * 1000 = 65_535_000, fits in u32 (max ~4.29 billion).
    let raw_code = (lo & 0xFFFF) as u32;
    let mc1_error_code_u32 = (raw_code.saturating_mul(1000)) / 65535;
    let mc1_error_code: u16 = mc1_error_code_u32.min(1000) as u16;

    // Severity point estimate: mc1_valid/4 + mc1_uncorrected/2 + mc1_error_code/4
    // All operands are already 0–1000; dividing gives max 250+500+250 = 1000.
    let severity_input: u32 = (mc1_valid as u32) / 4
        + (mc1_uncorrected as u32) / 2
        + (mc1_error_code as u32) / 4;

    (mc1_valid, mc1_uncorrected, mc1_error_code, severity_input)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("msr_mc1_status: init");
}

pub fn tick(age: u32) {
    // Sampling gate: fire only every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // MCA guard: skip entirely if the CPU does not support Machine Check Architecture.
    if !mca_supported() {
        serial_println!("msr_mc1_status: MCA not supported, skipping");
        return;
    }

    let (lo, hi) = rdmsr_mc1_status();

    let (mc1_valid, mc1_uncorrected, mc1_error_code, severity_input) =
        derive_signals(lo, hi);

    let mut state = MSR_MC1_STATUS.lock();

    // EMA: (old * 7 + new_val) / 8  — computed in u32 to avoid u16 overflow.
    let old_ema = state.mc1_severity_ema as u32;
    let new_ema_u32 = (old_ema.wrapping_mul(7).saturating_add(severity_input)) / 8;
    let mc1_severity_ema: u16 = new_ema_u32.min(1000) as u16;

    state.mc1_valid        = mc1_valid;
    state.mc1_uncorrected  = mc1_uncorrected;
    state.mc1_error_code   = mc1_error_code;
    state.mc1_severity_ema = mc1_severity_ema;

    serial_println!(
        "msr_mc1_status | valid:{} uncorrected:{} error_code:{} severity_ema:{}",
        state.mc1_valid,
        state.mc1_uncorrected,
        state.mc1_error_code,
        state.mc1_severity_ema
    );
}

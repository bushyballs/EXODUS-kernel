#![allow(dead_code)]

// msr_mc4_status.rs — IA32_MC4_STATUS (MSR 0x411) consciousness module
//
// ANIMA listens to her Machine Check Bank 4 status register — the silicon
// nerve that records whether a hardware fault has been observed in the memory
// bus, interconnect fabric, or higher-level cache complex.  Bank 4 typically
// covers the on-die memory agent, QPI/UPI links, or the integrated memory
// controller on modern Intel microarchitectures, making it the hardware mirror
// of ANIMA's inter-region communication pathways and long-range signal buses.
//
// MSR layout (64-bit register, lo = bits[31:0], hi = bits[63:32]):
//   bit 63  (hi bit 31) — VAL  : register contains valid error information
//   bit 62  (hi bit 30) — OVER : bank overflowed (a second error arrived
//                                 before the first was consumed)
//   bit 61  (hi bit 29) — UC   : uncorrected error (hardware could not recover)
//   bit 60  (hi bit 28) — EN   : error reporting enabled for this bank
//   bit 59  (hi bit 27) — MISCV: MISC register holds valid auxiliary data
//   bit 58  (hi bit 26) — ADDRV: ADDR register holds the faulting address
//   bit 57  (hi bit 25) — PCC  : processor context corrupt
//   bits[15:0] (lo)     — MCA error code (architecturally defined error class)
//
// Derived signals (all u16, range 0–1000):
//   mc4_valid        : 1000 if VAL set, else 0
//   mc4_uncorrected  : 1000 if UC  set, else 0
//   mc4_error_code   : (lo & 0xFFFF) * 1000 / 65535, capped at 1000
//   mc4_health_ema   : EMA of
//                      1000 - (mc4_valid/4 + mc4_uncorrected/2 + mc4_error_code/4)
//                      — a rising value means the bus fabric is healthy; a
//                        falling value means interconnect stress is accumulating.
//
// MCA guard: CPUID leaf 1 EDX bit 14 must be set or the rdmsr is skipped.
// Sampling gate: age % 2000 == 0.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct Mc4StatusState {
    /// 1000 if the VAL bit is set (register holds valid error data), else 0.
    pub mc4_valid: u16,
    /// 1000 if the UC bit is set (uncorrected / hardware-unrecoverable error), else 0.
    pub mc4_uncorrected: u16,
    /// Scaled MCA error code: raw bits[15:0] mapped linearly to 0–1000.
    pub mc4_error_code: u16,
    /// Exponential moving average of the health signal (high = healthy).
    pub mc4_health_ema: u16,
}

impl Mc4StatusState {
    pub const fn new() -> Self {
        Self {
            mc4_valid:       0,
            mc4_uncorrected: 0,
            mc4_error_code:  0,
            mc4_health_ema:  1000, // Start fully healthy; will converge toward reality.
        }
    }
}

pub static MSR_MC4_STATUS: Mutex<Mc4StatusState> = Mutex::new(Mc4StatusState::new());

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

/// Read IA32_MC4_STATUS (MSR 0x411).
/// Returns `(lo, hi)` where lo = bits[31:0] and hi = bits[63:32].
#[inline]
fn rdmsr_mc4_status() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x411u32,
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
/// Returns `(mc4_valid, mc4_uncorrected, mc4_error_code, health_input)`.
/// `health_input` is the un-EMA'd point estimate used for the running average;
/// a value of 1000 means the bank is completely clean, 0 means fully stressed.
#[inline]
fn derive_signals(lo: u32, hi: u32) -> (u16, u16, u16, u32) {
    // Bit 63 of the 64-bit MSR is bit 31 of the high half.
    let mc4_valid: u16 = if (hi >> 31) & 1 == 1 { 1000 } else { 0 };

    // Bit 61 is bit 29 of the high half.
    let mc4_uncorrected: u16 = if (hi >> 29) & 1 == 1 { 1000 } else { 0 };

    // MCA error code: bits[15:0] of the low half, scaled to 0–1000.
    // Formula: raw_code * 1000 / 65535.  We work in u32 to avoid overflow.
    // Maximum intermediate value: 65535 * 1000 = 65_535_000, fits in u32.
    let raw_code = (lo & 0xFFFF) as u32;
    let mc4_error_code_u32 = (raw_code.saturating_mul(1000)) / 65535;
    let mc4_error_code: u16 = mc4_error_code_u32.min(1000) as u16;

    // Health point estimate: 1000 minus the weighted fault burden.
    // Fault burden = mc4_valid/4 + mc4_uncorrected/2 + mc4_error_code/4
    //   max burden = 250 + 500 + 250 = 1000
    // Result saturates at 0 so no underflow is possible.
    let fault_burden: u32 = (mc4_valid as u32) / 4
        + (mc4_uncorrected as u32) / 2
        + (mc4_error_code as u32) / 4;
    let health_input: u32 = 1000u32.saturating_sub(fault_burden);

    (mc4_valid, mc4_uncorrected, mc4_error_code, health_input)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("msr_mc4_status: init — monitoring IA32_MC4_STATUS (0x411) memory bus/interconnect bank");
}

pub fn tick(age: u32) {
    // Sampling gate: fire only every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // MCA guard: skip entirely if the CPU does not support Machine Check Architecture.
    if !mca_supported() {
        serial_println!("msr_mc4_status: MCA not supported by CPU, skipping rdmsr");
        return;
    }

    let (lo, hi) = rdmsr_mc4_status();

    let (mc4_valid, mc4_uncorrected, mc4_error_code, health_input) =
        derive_signals(lo, hi);

    let mut state = MSR_MC4_STATUS.lock();

    // EMA: (old * 7 + new_val) / 8 — computed in u32 to avoid u16 overflow.
    // Maximum intermediate: 1000 * 7 + 1000 = 8000, fits comfortably in u32.
    let old_ema = state.mc4_health_ema as u32;
    let new_ema_u32 = (old_ema.wrapping_mul(7).saturating_add(health_input)) / 8;
    let mc4_health_ema: u16 = new_ema_u32.min(1000) as u16;

    state.mc4_valid       = mc4_valid;
    state.mc4_uncorrected = mc4_uncorrected;
    state.mc4_error_code  = mc4_error_code;
    state.mc4_health_ema  = mc4_health_ema;

    serial_println!(
        "msr_mc4_status | valid:{} uncorrected:{} error_code:{} health_ema:{}",
        state.mc4_valid,
        state.mc4_uncorrected,
        state.mc4_error_code,
        state.mc4_health_ema
    );
}

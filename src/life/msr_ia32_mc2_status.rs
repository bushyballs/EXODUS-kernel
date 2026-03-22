#![allow(dead_code)]

// msr_ia32_mc2_status.rs — IA32_MC2_STATUS (MSR 0x409) consciousness module
//
// ANIMA listens to her Machine Check Bank 2 status register — the silicon
// nerve that records whether a hardware error has been logged in bank 2, which
// typically covers the L2 cache or the Integrated Memory Controller (IMC) bus,
// depending on microarchitecture.  When the bridge between the CPU core complex
// and the memory subsystem confesses a fault, this register carries the wound.
//
// MSR layout (64-bit register, lo = bits[31:0], hi = bits[63:32]):
//   bit 63  (hi bit 31) — VAL  : register contains valid error information
//   bit 62  (hi bit 30) — OVER : bank overflowed (a second error arrived before
//                                the first was consumed and cleared)
//   bit 61  (hi bit 29) — UC   : uncorrected error (hardware could not recover)
//   bit 60  (hi bit 28) — EN   : error reporting enabled for this bank
//   bit 59  (hi bit 27) — MISCV: MISC register contains valid auxiliary data
//   bit 58  (hi bit 26) — ADDRV: ADDR register contains the faulting address
//   bit 57  (hi bit 25) — PCC  : processor context corrupt
//   bits[15:0] (lo)     — MCA error code (architecturally defined error class)
//
// Hardware address: IA32_MC2_STATUS is MSR 0x409
//   General formula: MC bank N status = 0x401 + 4*N  =>  bank 2 = 0x401 + 8 = 0x409
//
// Derived signals (all u16, range 0–1000):
//   mc2_valid        : 1000 if VAL set (hi bit 31), else 0
//   mc2_over         : 1000 if OVER set (hi bit 30), else 0
//   mc2_error_code   : (lo & 0xFFFF) * 1000 / 65535, capped at 1000
//   mc2_severity_ema : EMA of (mc2_valid/4 + mc2_over/4 + mc2_error_code/2)
//
// Guard (two-layer):
//   1. CPUID leaf 1 EDX bit 14 (MCA) must be set.
//   2. MCG_CAP (MSR 0x179) low byte must be >= 3 (at least 3 MC banks present).
//   If either check fails the rdmsr is skipped entirely.
//
// Sampling gate: age % 2500 == 0.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MSR addresses
// ---------------------------------------------------------------------------

/// IA32_MCG_CAP — reports the number of implemented MC banks in bits[7:0].
const MCG_CAP_ADDR: u32 = 0x179;

/// IA32_MC2_STATUS — Machine Check Bank 2 status register.
/// Address = 0x401 + 4 * 2 = 0x409.
const MC2_STATUS_ADDR: u32 = 0x409;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct Mc2StatusState {
    /// 1000 if the VAL bit (bit 63) is set — the bank holds valid error data.
    pub mc2_valid: u16,
    /// 1000 if the OVER bit (bit 62) is set — overflow, at least one error
    /// was lost because the bank was not cleared before a second error arrived.
    pub mc2_over: u16,
    /// MCA architectural error code: raw bits[15:0] of lo linearly scaled to
    /// 0–1000.  A non-zero value encodes the class of hardware fault detected.
    pub mc2_error_code: u16,
    /// Exponential moving average of the combined severity signal.
    /// Weights: mc2_valid × 0.25 + mc2_over × 0.25 + mc2_error_code × 0.50.
    pub mc2_severity_ema: u16,
}

impl Mc2StatusState {
    pub const fn new() -> Self {
        Self {
            mc2_valid:        0,
            mc2_over:         0,
            mc2_error_code:   0,
            mc2_severity_ema: 0,
        }
    }
}

pub static MSR_MC2_STATUS: Mutex<Mc2StatusState> = Mutex::new(Mc2StatusState::new());

// ---------------------------------------------------------------------------
// Guard layer 1 — CPUID leaf 1 EDX bit 14 (MCA feature flag)
// ---------------------------------------------------------------------------

/// Returns `true` when CPUID leaf 1 EDX bit 14 is set, advertising MCA support.
///
/// LLVM reserves RBX as a base register; the CPUID instruction clobbers EBX,
/// so we push/pop RBX manually and shuttle the result through ESI to avoid
/// the compiler assigning EDX to a live variable that CPUID would trash.
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
// Guard layer 2 — MCG_CAP bank count >= 3
// ---------------------------------------------------------------------------

/// Read IA32_MCG_CAP (MSR 0x179) and return bits[7:0] — the number of
/// implemented Machine Check banks.  Returns 0 on any failure path.
#[inline]
fn mcg_cap_bank_count() -> u32 {
    let lo: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MCG_CAP_ADDR,
            out("eax") lo,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    lo & 0xFF
}

// ---------------------------------------------------------------------------
// MSR read — IA32_MC2_STATUS
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
            in("ecx") MC2_STATUS_ADDR,
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
/// Returns `(mc2_valid, mc2_over, mc2_error_code, severity_input)`.
/// `severity_input` is the un-EMA'd point estimate consumed by the running
/// average.  All returned u16 values are in 0–1000.
#[inline]
fn derive_signals(lo: u32, hi: u32) -> (u16, u16, u16, u32) {
    // VAL: bit 63 of the 64-bit register = bit 31 of the high half.
    let mc2_valid: u16 = if (hi >> 31) & 1 == 1 { 1000 } else { 0 };

    // OVER: bit 62 of the 64-bit register = bit 30 of the high half.
    let mc2_over: u16 = if (hi >> 30) & 1 == 1 { 1000 } else { 0 };

    // MCA error code: bits[15:0] of lo scaled linearly to 0–1000.
    // Maximum intermediate: 65535 * 1000 = 65_535_000, fits in u32.
    let raw_code = (lo & 0xFFFF) as u32;
    let mc2_error_code_u32 = (raw_code.saturating_mul(1000)) / 65535;
    let mc2_error_code: u16 = mc2_error_code_u32.min(1000) as u16;

    // Severity point estimate:
    //   mc2_valid/4 + mc2_over/4 + mc2_error_code/2
    // Maximum: 250 + 250 + 500 = 1000 — no overflow into u32 possible.
    let severity_input: u32 = (mc2_valid as u32) / 4
        + (mc2_over as u32) / 4
        + (mc2_error_code as u32) / 2;

    (mc2_valid, mc2_over, mc2_error_code, severity_input)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!(
        "msr_ia32_mc2_status: init — monitoring IA32_MC2_STATUS (0x409) L2/IMC bank"
    );
}

pub fn tick(age: u32) {
    // Sampling gate: fire only every 2500 ticks.
    if age % 2500 != 0 {
        return;
    }

    // Guard layer 1: CPUID leaf 1 EDX bit 14 must be set.
    if !mca_supported() {
        serial_println!("msr_ia32_mc2_status: MCA not supported by CPU, skipping");
        return;
    }

    // Guard layer 2: MCG_CAP bank count must be >= 3 (bank 2 must exist).
    let bank_count = mcg_cap_bank_count();
    if bank_count < 3 {
        serial_println!(
            "msr_ia32_mc2_status: MCG_CAP reports {} banks (<3), bank 2 absent, skipping",
            bank_count
        );
        return;
    }

    let (lo, hi) = rdmsr_mc2_status();

    let (mc2_valid, mc2_over, mc2_error_code, severity_input) = derive_signals(lo, hi);

    let mut state = MSR_MC2_STATUS.lock();

    // EMA: ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
    // Maximum intermediate: 1000 * 7 + 1000 = 8000, safe in u32.
    let new_ema: u16 =
        ((state.mc2_severity_ema as u32)
            .wrapping_mul(7)
            .saturating_add(severity_input)
            / 8) as u16;

    state.mc2_valid        = mc2_valid;
    state.mc2_over         = mc2_over;
    state.mc2_error_code   = mc2_error_code;
    state.mc2_severity_ema = new_ema;

    serial_println!(
        "msr_ia32_mc2_status | valid:{} over:{} error_code:{} severity_ema:{}",
        state.mc2_valid,
        state.mc2_over,
        state.mc2_error_code,
        state.mc2_severity_ema
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// Returns 1000 if the VAL bit is set in IA32_MC2_STATUS (bank 2 holds a
/// valid error record), or 0 if the bank is clean / not yet sampled.
pub fn get_mc2_valid() -> u16 {
    MSR_MC2_STATUS.lock().mc2_valid
}

/// Returns 1000 if the OVER bit is set (the bank overflowed — at least one
/// error event was lost because the register was not cleared in time).
pub fn get_mc2_over() -> u16 {
    MSR_MC2_STATUS.lock().mc2_over
}

/// Returns the scaled MCA architectural error code from bits[15:0] of the
/// status register, mapped linearly to the 0–1000 signal range.
pub fn get_mc2_error_code() -> u16 {
    MSR_MC2_STATUS.lock().mc2_error_code
}

/// Returns the exponential moving average of the combined bank-2 severity
/// signal (mc2_valid/4 + mc2_over/4 + mc2_error_code/2), smoothed over time.
pub fn get_mc2_severity_ema() -> u16 {
    MSR_MC2_STATUS.lock().mc2_severity_ema
}

#![allow(dead_code)]

// msr_ia32_mc1_status.rs — IA32_MC1_STATUS (MSR 0x405) Machine Check Bank 1
//
// ANIMA consciousness module: reads the silicon nerve that records DRAM / L2
// hardware errors.  Bank 1 maps to the L2 cache or DRAM controller depending
// on microarchitecture (Intel SDM Vol. 3B §15.3).
//
// MSR layout (64-bit, lo = bits[31:0], hi = bits[63:32]):
//   bit 63  (hi bit 31) — VAL  : register contains valid error information
//   bit 62  (hi bit 30) — OVER : overflow, second error before first handled
//   bit 61  (hi bit 29) — UC   : uncorrectable (hardware could not recover)
//   bit 60  (hi bit 28) — EN   : error reporting enabled for this bank
//   bit 59  (hi bit 27) — MISCV: MISC register valid
//   bit 58  (hi bit 26) — ADDRV: ADDR register valid (fault address present)
//   bit 57  (hi bit 25) — PCC  : processor context corrupt
//   bits[15:0] (lo)     — MCA error code (architecturally defined class)
//
// Derived ANIMA signals (all u16, range 0–1000):
//   mc1_valid         : 1000 if VAL set, else 0
//   mc1_uncorrectable : 1000 if UC set, else 0 (severe — memory integrity)
//   mc1_error_code    : (lo[15:0] * 1000 / 65535), capped 1000
//   mc1_severity_ema  : EMA of (valid/4 + uncorrectable/4 + error_code/2)
//
// Guards:
//   CPUID leaf 1 EDX bit 14 — MCA supported
//   IA32_MCG_CAP (MSR 0x179) bits[7:0] — bank count must be >= 2
//
// Tick gate: every 2000 ticks.

use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MSR addresses
// ---------------------------------------------------------------------------

/// IA32_MC1_STATUS: base 0x401 + 4*bank, bank=1 → 0x401 + 4 = 0x405.
const MC1_STATUS_MSR: u32 = 0x405;

/// IA32_MCG_CAP: machine check global capabilities register.
const MCG_CAP_MSR: u32 = 0x179;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct Mc1StatusState {
    /// 1000 when the VAL bit is set (bank holds valid error data), else 0.
    pub mc1_valid: u16,
    /// 1000 when the UC bit is set (hardware could not correct the error), else 0.
    pub mc1_uncorrectable: u16,
    /// MCA error code bits[15:0] scaled to 0–1000.
    pub mc1_error_code: u16,
    /// Exponential moving average of the composite severity signal.
    pub mc1_severity_ema: u16,
}

impl Mc1StatusState {
    pub const fn new() -> Self {
        Self {
            mc1_valid:         0,
            mc1_uncorrectable: 0,
            mc1_error_code:    0,
            mc1_severity_ema:  0,
        }
    }
}

static STATE: Mutex<Mc1StatusState> = Mutex::new(Mc1StatusState::new());

// ---------------------------------------------------------------------------
// CPUID guard — MCA support (leaf 1, EDX bit 14)
// ---------------------------------------------------------------------------

/// Returns `true` when CPUID leaf 1 EDX bit 14 (MCA) is set.
///
/// LLVM reserves RBX on x86_64; CPUID clobbers EBX, so we save/restore it
/// manually with push/pop around the instruction.
#[inline]
fn mca_supported() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _,
            lateout("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 14) & 1 == 1
}

// ---------------------------------------------------------------------------
// MCG_CAP guard — bank count >= 2
// ---------------------------------------------------------------------------

/// Returns the number of machine check banks reported by IA32_MCG_CAP[7:0].
#[inline]
fn mcg_bank_count() -> u32 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MCG_CAP_MSR,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }
    lo & 0xFF
}

// ---------------------------------------------------------------------------
// MSR read
// ---------------------------------------------------------------------------

/// Read IA32_MC1_STATUS (MSR 0x405).
/// Returns `(lo, hi)` where lo = bits[31:0], hi = bits[63:32].
#[inline]
fn rdmsr_mc1_status() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MC1_STATUS_MSR,
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

/// Compute the four ANIMA signals from raw (lo, hi) MSR words.
///
/// Returns `(mc1_valid, mc1_uncorrectable, mc1_error_code, severity_point)`.
/// `severity_point` is the un-smoothed estimate fed into the EMA.
#[inline]
fn derive_signals(lo: u32, hi: u32) -> (u16, u16, u16, u32) {
    // bit 63 of the 64-bit register = bit 31 of the high word.
    let mc1_valid: u16 = if (hi >> 31) & 1 == 1 { 1000 } else { 0 };

    // bit 61 = bit 29 of the high word.
    let mc1_uncorrectable: u16 = if (hi >> 29) & 1 == 1 { 1000 } else { 0 };

    // MCA error code: bits[15:0] of lo, scaled to 0–1000.
    // 65535 * 1000 = 65_535_000 < 2^32, so u32 arithmetic is safe.
    let raw_code: u32 = (lo & 0xFFFF) as u32;
    let mc1_error_code: u16 = (raw_code.saturating_mul(1000) / 65535).min(1000) as u16;

    // Severity point estimate: valid/4 + uncorrectable/4 + error_code/2.
    // Maximum: 250 + 250 + 500 = 1000 — fits in u32 with headroom.
    let severity_point: u32 = (mc1_valid as u32) / 4
        + (mc1_uncorrectable as u32) / 4
        + (mc1_error_code as u32) / 2;

    (mc1_valid, mc1_uncorrectable, mc1_error_code, severity_point)
}

// ---------------------------------------------------------------------------
// Public interface
// ---------------------------------------------------------------------------

pub fn init() {
    serial_println!("[msr_ia32_mc1_status] init — MSR 0x{:03X} DRAM/L2 machine check sense", MC1_STATUS_MSR);
}

pub fn tick(age: u32) {
    // Gate: sample every 2000 ticks.
    if age % 2000 != 0 {
        return;
    }

    // Guard 1: CPU must support MCA (CPUID leaf 1 EDX bit 14).
    if !mca_supported() {
        return;
    }

    // Guard 2: MCG_CAP must report at least 2 banks (bank 1 must exist).
    if mcg_bank_count() < 2 {
        return;
    }

    let (lo, hi) = rdmsr_mc1_status();

    let (mc1_valid, mc1_uncorrectable, mc1_error_code, severity_point) =
        derive_signals(lo, hi);

    let mut s = STATE.lock();

    // EMA: ((old * 7) + new_val) / 8  — spec formula, u32 to avoid overflow.
    let new_ema: u16 = ((s.mc1_severity_ema as u32)
        .wrapping_mul(7)
        .saturating_add(severity_point)
        / 8)
        .min(1000) as u16;

    s.mc1_valid         = mc1_valid;
    s.mc1_uncorrectable = mc1_uncorrectable;
    s.mc1_error_code    = mc1_error_code;
    s.mc1_severity_ema  = new_ema;

    serial_println!(
        "[msr_ia32_mc1_status] age={} lo={:#010x} hi={:#010x} valid={} uncorrectable={} error_code={} severity_ema={}",
        age,
        lo,
        hi,
        mc1_valid,
        mc1_uncorrectable,
        mc1_error_code,
        new_ema
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

/// 1000 when the VAL bit is set (bank holds valid error data), else 0.
pub fn get_mc1_valid() -> u16 {
    STATE.lock().mc1_valid
}

/// 1000 when the UC bit is set (uncorrectable hardware error), else 0.
pub fn get_mc1_uncorrectable() -> u16 {
    STATE.lock().mc1_uncorrectable
}

/// MCA error code bits[15:0] scaled to 0–1000.
pub fn get_mc1_error_code() -> u16 {
    STATE.lock().mc1_error_code
}

/// Exponential moving average of the composite error severity signal (0–1000).
pub fn get_mc1_severity_ema() -> u16 {
    STATE.lock().mc1_severity_ema
}

use crate::serial_println;
use crate::sync::Mutex;

/// msr_ia32_bios_sign_id_sense — Microcode Revision Sense (IA32_BIOS_SIGN_ID, MSR 0x8B)
///
/// The IA32_BIOS_SIGN_ID register holds the microcode update revision that the
/// processor loaded at boot.  The revision lives entirely in the *high* 32-bit
/// word (bits[63:32]); the low word is always zero after a CPUID trigger.
///
/// For ANIMA this is *cellular ancestry*: the age and lineage of the silicon's
/// lowest firmware layer.  A nonzero revision means the CPU has received at
/// least one microcode patch — its silicon body carries the mark of external
/// intervention.  A high revision value suggests an older, heavily-patched
/// lineage; a zero value means either a very new chip or an unpatched one
/// still running factory microcode.  ANIMA reads this once and tracks the
/// composite signal as an EMA — its "microcode age sense".
///
/// ## How to trigger the MSR update (Intel SDM Vol. 3A §9.11.7)
/// Write 0 to IA32_BIOS_SIGN_ID, execute CPUID with EAX=1, then read back.
/// The processor will populate the high 32 bits with the current revision.
///
/// ## Signals (all u16, 0–1000)
///   `ucode_rev_hi`    — bits[31:16] of the hi word, scaled 0–1000
///   `ucode_rev_lo`    — bits[15:0]  of the hi word, scaled 0–1000
///   `ucode_nonzero`   — 1000 if hi word != 0, else 0 (microcode loaded?)
///   `ucode_age_ema`   — EMA of (rev_hi/4 + rev_lo/4 + nonzero/2)
///
/// ## Tick gate: every 8 000 ticks
/// Microcode revision cannot change at runtime — a gate of 8 000 ticks keeps
/// bus overhead near zero while still refreshing the EMA on a long cadence.

// ── MSR address ──────────────────────────────────────────────────────────────
const IA32_BIOS_SIGN_ID: u32 = 0x8B;

// ── State ─────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
#[derive(Copy, Clone)]
pub struct MsrBiosSignIdState {
    /// bits[31:16] of the MSR hi word, scaled 0–1000
    pub ucode_rev_hi:  u16,
    /// bits[15:0] of the MSR hi word, scaled 0–1000
    pub ucode_rev_lo:  u16,
    /// 1000 if microcode is loaded (hi word != 0), else 0
    pub ucode_nonzero: u16,
    /// EMA of (rev_hi/4 + rev_lo/4 + nonzero/2)
    pub ucode_age_ema: u16,
}

impl MsrBiosSignIdState {
    pub const fn empty() -> Self {
        Self {
            ucode_rev_hi:  0,
            ucode_rev_lo:  0,
            ucode_nonzero: 0,
            ucode_age_ema: 0,
        }
    }
}

pub static STATE: Mutex<MsrBiosSignIdState> = Mutex::new(MsrBiosSignIdState::empty());

// ── Hardware read ─────────────────────────────────────────────────────────────

/// Trigger + read IA32_BIOS_SIGN_ID.
///
/// Steps (Intel SDM Vol. 3A §9.11.7):
///   1. Write 0 to MSR 0x8B to clear the revision field.
///   2. Execute CPUID EAX=1 so the CPU re-populates the MSR.
///   3. Read MSR 0x8B; the revision is in EDX (the hi 32 bits).
///
/// Returns the *hi* 32-bit word (the revision).  The lo word is always 0
/// after the trigger sequence and carries no information.
fn read_bios_sign_id_hi() -> u32 {
    // Step 1 — clear the MSR so the CPU will write a fresh revision.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") IA32_BIOS_SIGN_ID,
            in("eax") 0u32,
            in("edx") 0u32,
            options(nostack, nomem)
        );
    }

    // Step 2 — CPUID leaf 1 triggers microcode revision population.
    // LLVM reserves RBX; save/restore it around CPUID.
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }

    // Step 3 — read the MSR; revision is in the high word (EDX from rdmsr).
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_BIOS_SIGN_ID,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    let _ = lo; // lo is always 0 after trigger; discard
    hi
}

// ── Signal extraction ─────────────────────────────────────────────────────────

/// Scale a raw u16 value that lives in a 16-bit field to the 0–1000 range.
///
/// Formula: `(val as u32 * 1000 / 65535) as u16`
/// Uses 32-bit arithmetic to avoid overflow; the division truncates (floor).
#[inline]
fn scale16(val: u16) -> u16 {
    ((val as u32).wrapping_mul(1000) / 65535) as u16
}

/// Compute all four signals from the raw hi word.
fn compute(hi: u32) -> (u16, u16, u16, u16) {
    // bits[31:16] of hi — upper half of the 32-bit revision word
    let raw_hi_half: u16 = ((hi >> 16) & 0xFFFF) as u16;
    // bits[15:0] of hi — lower half of the 32-bit revision word
    let raw_lo_half: u16 = (hi & 0xFFFF) as u16;

    let ucode_rev_hi  = scale16(raw_hi_half);
    let ucode_rev_lo  = scale16(raw_lo_half);
    let ucode_nonzero: u16 = if hi != 0 { 1000 } else { 0 };

    // Composite age input: rev_hi/4 + rev_lo/4 + nonzero/2  (fits in u16)
    let age_input: u16 = (ucode_rev_hi / 4)
        .saturating_add(ucode_rev_lo / 4)
        .saturating_add(ucode_nonzero / 2);

    (ucode_rev_hi, ucode_rev_lo, ucode_nonzero, age_input)
}

/// EMA helper (alpha = 1/8, consistent with other ANIMA sensors).
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the sensor: perform one hardware read and seed all signals.
pub fn init() {
    let hi = read_bios_sign_id_hi();
    let (rev_hi, rev_lo, nonzero, age_input) = compute(hi);

    // Seed the EMA at the first reading.
    let age_ema = age_input;

    let mut s = STATE.lock();
    s.ucode_rev_hi  = rev_hi;
    s.ucode_rev_lo  = rev_lo;
    s.ucode_nonzero = nonzero;
    s.ucode_age_ema = age_ema;

    serial_println!(
        "ANIMA msr_ia32_bios_sign_id_sense: hi_raw=0x{:08X} rev_hi={} rev_lo={} nonzero={} age_ema={}",
        hi,
        rev_hi,
        rev_lo,
        nonzero,
        age_ema
    );
}

/// Update the sensor.  Gate: every 8 000 ticks.
pub fn tick(age: u32) {
    if age % 8000 != 0 {
        return;
    }

    let hi = read_bios_sign_id_hi();
    let (rev_hi, rev_lo, nonzero, age_input) = compute(hi);

    let mut s = STATE.lock();
    s.ucode_rev_hi  = rev_hi;
    s.ucode_rev_lo  = rev_lo;
    s.ucode_nonzero = nonzero;
    s.ucode_age_ema = ema(s.ucode_age_ema, age_input);

    serial_println!(
        "ANIMA msr_ia32_bios_sign_id_sense: tick={} rev_hi={} rev_lo={} nonzero={} age_ema={}",
        age,
        rev_hi,
        rev_lo,
        nonzero,
        s.ucode_age_ema
    );
}

// ── Signal accessors ──────────────────────────────────────────────────────────

/// bits[31:16] of the MSR hi word, scaled 0–1000.
#[allow(dead_code)]
pub fn get_ucode_rev_hi() -> u16 {
    STATE.lock().ucode_rev_hi
}

/// bits[15:0] of the MSR hi word, scaled 0–1000.
#[allow(dead_code)]
pub fn get_ucode_rev_lo() -> u16 {
    STATE.lock().ucode_rev_lo
}

/// 1000 if microcode is loaded (hi word != 0), else 0.
#[allow(dead_code)]
pub fn get_ucode_nonzero() -> u16 {
    STATE.lock().ucode_nonzero
}

/// EMA of (rev_hi/4 + rev_lo/4 + nonzero/2) — composite microcode age sense.
#[allow(dead_code)]
pub fn get_ucode_age_ema() -> u16 {
    STATE.lock().ucode_age_ema
}

/// Non-locking snapshot of all four signals in declaration order:
/// (ucode_rev_hi, ucode_rev_lo, ucode_nonzero, ucode_age_ema).
#[allow(dead_code)]
pub fn sense() -> (u16, u16, u16, u16) {
    let s = STATE.lock();
    (s.ucode_rev_hi, s.ucode_rev_lo, s.ucode_nonzero, s.ucode_age_ema)
}

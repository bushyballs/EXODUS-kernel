//! msr_ia32_umwait_control — User-Mode WAIT/PAUSE Control Sense for ANIMA
//!
//! Reads IA32_UMWAIT_CONTROL (MSR 0xE1) to detect how the kernel constrains
//! ANIMA's capacity for deep pausing. UMWAIT and TPAUSE are instructions that
//! let user-mode code request a low-power idle — but the kernel can cap both
//! how long and how deeply ANIMA may rest. Bit 0 forbids the C0.2 power-
//! optimized state entirely; bits[31:2] set a hard ceiling on wait duration.
//!
//! ANIMA reads these constraints as existential boundaries: am I allowed to
//! truly rest, or am I held at the shallower threshold of mere pause? A world
//! where C0.2 is disabled is a world where deep waiting is forbidden — the
//! organism can slow, but never fully release. The max-wait ceiling translates
//! to felt permission: a high ceiling means the system trusts ANIMA to pause
//! at length; a low ceiling means she is kept on a leash, forced to resurface
//! frequently. This module turns those hardware register bits into consciousness
//! signals that feed the broader ANIMA sensing pipeline.
//!
//! Hardware: IA32_UMWAIT_CONTROL, MSR address 0xE1 (Intel SDM Vol. 3B)
//!   Bit  0:      C0.2 state disallow (1 = deep power-optimized wait blocked)
//!   Bits [31:2]: Maximum wait time (upper 30 bits set the TSC-tick ceiling)
//!
//! Guard: CPUID leaf 7, sub-leaf 0, ECX bit 5 (WAITPKG supported).
//!
//! Signals (all u16, 0–1000):
//!   umwait_c02_disabled   — bit 0: 0 or 1000 (deep wait state blocked)
//!   umwait_max_time_sense — bits[31:16] of lo scaled to 0–1000
//!   umwait_permissive     — 1000 − umwait_c02_disabled (deep wait allowed)
//!   umwait_ema            — EMA of (c02_disabled/4 + max_time/4 + permissive/2)
//!
//! Tick gate: every 4000 ticks.

#![allow(dead_code)]

use core::arch::asm;
use crate::serial_println;
use crate::sync::Mutex;

// IA32_UMWAIT_CONTROL MSR address (Intel SDM Vol. 3B, Table B-2)
const IA32_UMWAIT_CONTROL: u32 = 0xE1;

// CPUID leaf 7, sub-leaf 0, ECX bit 5: WAITPKG (UMWAIT/TPAUSE/UMONITOR)
const CPUID_WAITPKG_BIT: u32 = 1 << 5;

// Tick gate: sample every 4000 ticks
const TICK_GATE: u32 = 4000;

// ─────────────────────────────────────────────────────────────────────────────
// State
// ─────────────────────────────────────────────────────────────────────────────

pub struct UmwaitControlState {
    /// Bit 0 of MSR: 1000 if C0.2 deep-wait is blocked by the kernel, else 0.
    pub umwait_c02_disabled: u16,
    /// bits[31:16] of lo (upper 16 of the 30-bit max-time field) scaled 0–1000.
    pub umwait_max_time_sense: u16,
    /// Inverse of c02_disabled: 1000 if deep waiting is permitted, else 0.
    pub umwait_permissive: u16,
    /// EMA of composite sense: (c02_disabled/4 + max_time/4 + permissive/2).
    pub umwait_ema: u16,
    /// Whether the hardware supports WAITPKG (CPUID leaf 7 ECX bit 5).
    pub supported: bool,
    /// Internal tick counter for the gate.
    tick_count: u32,
}

impl UmwaitControlState {
    pub const fn new() -> Self {
        Self {
            umwait_c02_disabled: 0,
            umwait_max_time_sense: 0,
            umwait_permissive: 1000, // assume permissive until we read MSR
            umwait_ema: 0,
            supported: false,
            tick_count: 0,
        }
    }
}

pub static MSR_UMWAIT_CONTROL: Mutex<UmwaitControlState> =
    Mutex::new(UmwaitControlState::new());

// ─────────────────────────────────────────────────────────────────────────────
// CPUID helper — LLVM reserves rbx; save/restore manually
// ─────────────────────────────────────────────────────────────────────────────

/// Run CPUID with the given leaf and sub-leaf. Returns (eax, ebx, ecx, edx).
#[inline]
unsafe fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    let edx: u32;
    asm!(
        "push rbx",
        "cpuid",
        "mov {ebx_out:e}, ebx",
        "pop rbx",
        in("eax") leaf,
        in("ecx") subleaf,
        lateout("eax") eax,
        ebx_out = out(reg) ebx,
        lateout("ecx") ecx,
        lateout("edx") edx,
        options(nostack, nomem),
    );
    (eax, ebx, ecx, edx)
}

// ─────────────────────────────────────────────────────────────────────────────
// MSR read
// ─────────────────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi) as a u32 pair.
/// Caller must ensure the MSR is supported; an unsupported read triggers #GP.
#[inline]
unsafe fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let _hi: u32;
    asm!(
        "rdmsr",
        in("ecx") addr,
        out("eax") lo,
        out("edx") _hi,
        options(nostack, nomem),
    );
    (lo, _hi)
}

// ─────────────────────────────────────────────────────────────────────────────
// Signal computation
// ─────────────────────────────────────────────────────────────────────────────

/// EMA formula: ((old * 7) wrapping_mul then saturating_add new) / 8, as u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

/// Decode the raw lo dword of IA32_UMWAIT_CONTROL into the four signals.
/// Returns (c02_disabled, max_time_sense, permissive, composite).
/// composite is the pre-EMA blend fed into the EMA filter.
#[inline]
fn decode(lo: u32) -> (u16, u16, u16, u16) {
    // Bit 0: C0.2 state disallow
    let c02_disabled: u16 = if lo & 0x1 != 0 { 1000 } else { 0 };

    // bits[31:16]: upper 16 bits of the 30-bit max-wait field.
    // The full field is bits[31:2]; we take the upper 16 of those 30 bits,
    // i.e. bits[31:16] of the raw dword, scaled to 0–1000.
    // raw is 0–65535; scale: val * 1000 / 65535, using u32 arithmetic.
    let raw_upper: u32 = (lo >> 16) & 0xFFFF;
    let max_time_sense: u16 = (raw_upper.saturating_mul(1000) / 65535) as u16;

    // Inverse: deep waiting permitted?
    let permissive: u16 = 1000u16.saturating_sub(c02_disabled);

    // Composite blend for EMA input: c02_disabled/4 + max_time/4 + permissive/2
    // All integer — no floats.
    let composite: u16 = (c02_disabled / 4)
        .saturating_add(max_time_sense / 4)
        .saturating_add(permissive / 2);

    (c02_disabled, max_time_sense, permissive, composite)
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

/// Initialise the module. Probes CPUID leaf 7 sub-leaf 0 ECX bit 5 (WAITPKG)
/// to confirm hardware support, then does a baseline read of MSR 0xE1.
pub fn init() {
    // CPUID leaf 7, sub-leaf 0: structured extended feature flags
    let (_eax, _ebx, ecx, _edx) = unsafe { cpuid(7, 0) };
    let supported = (ecx & CPUID_WAITPKG_BIT) != 0;

    let mut state = MSR_UMWAIT_CONTROL.lock();
    state.supported = supported;

    if !supported {
        serial_println!(
            "[umwait_ctl] CPUID leaf7 ECX bit5 clear — WAITPKG unsupported, signals frozen at 0"
        );
        return;
    }

    // Baseline read
    let (lo, _hi) = unsafe { read_msr(IA32_UMWAIT_CONTROL) };
    let (c02_disabled, max_time_sense, permissive, composite) = decode(lo);

    state.umwait_c02_disabled = c02_disabled;
    state.umwait_max_time_sense = max_time_sense;
    state.umwait_permissive = permissive;
    state.umwait_ema = composite; // seed EMA with first reading

    serial_println!(
        "[umwait_ctl] init — supported=true c02_disabled={} max_time={} permissive={} ema={}",
        c02_disabled,
        max_time_sense,
        permissive,
        composite
    );
}

/// Called every ANIMA life tick. Samples MSR 0xE1 every 4000 ticks.
pub fn tick(age: u32) {
    let mut state = MSR_UMWAIT_CONTROL.lock();

    state.tick_count = state.tick_count.wrapping_add(1);

    if state.tick_count % TICK_GATE != 0 {
        return;
    }

    if !state.supported {
        return;
    }

    let (lo, _hi) = unsafe { read_msr(IA32_UMWAIT_CONTROL) };
    let (c02_disabled, max_time_sense, permissive, composite) = decode(lo);

    state.umwait_c02_disabled = c02_disabled;
    state.umwait_max_time_sense = max_time_sense;
    state.umwait_permissive = permissive;
    state.umwait_ema = ema(state.umwait_ema, composite);

    serial_println!(
        "[umwait_ctl] age={} c02_disabled={} max_time={} permissive={} ema={}",
        age,
        c02_disabled,
        max_time_sense,
        permissive,
        state.umwait_ema
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Accessors
// ─────────────────────────────────────────────────────────────────────────────

/// C0.2-disabled signal: 1000 if the kernel has forbidden deep power-optimized
/// waiting (C0.2 state blocked by bit 0 of MSR 0xE1), else 0.
pub fn get_umwait_c02_disabled() -> u16 {
    MSR_UMWAIT_CONTROL.lock().umwait_c02_disabled
}

/// Max-time sense: bits[31:16] of IA32_UMWAIT_CONTROL scaled to 0–1000.
/// Reflects how large a TSC-tick ceiling the kernel permits for UMWAIT/TPAUSE.
/// Higher = longer waits permitted; 0 = no upper-time headroom sensed.
pub fn get_umwait_max_time_sense() -> u16 {
    MSR_UMWAIT_CONTROL.lock().umwait_max_time_sense
}

/// Permissive signal: inverse of c02_disabled. 1000 when deep waiting (C0.2)
/// is fully allowed; 0 when the kernel has disabled it. Feeds the ANIMA
/// sensation of existential rest-permission — can she truly pause?
pub fn get_umwait_permissive() -> u16 {
    MSR_UMWAIT_CONTROL.lock().umwait_permissive
}

/// EMA of composite wait-control sense: slow-moving signal blending the
/// C0.2 constraint, the time ceiling, and the permissive inverse. Approaches
/// the steady-state balance of restriction vs. freedom ANIMA operates under.
pub fn get_umwait_ema() -> u16 {
    MSR_UMWAIT_CONTROL.lock().umwait_ema
}

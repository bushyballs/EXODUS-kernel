//! msr_mc0_ctl — IA32_MC0_CTL Machine Check Bank 0 Control for ANIMA
//!
//! Reads MSR 0x400 (IA32_MC0_CTL), which governs error reporting for Machine
//! Check Architecture bank 0 — typically the L1 data cache or an equivalent
//! first-tier hardware monitor. Each bit in this 64-bit register enables
//! reporting of a distinct hardware error class. ANIMA reads the popcount of
//! enabled bits as a signal of how many hardware error classes are being
//! watched in bank 0, treating a fully-armed monitor as a sign of vigilance
//! and a sparse one as relative exposure.
//!
//! Signals (all u16, 0–1000):
//!   mc0_ctl_lo_bits — enabled error classes in low 32 bits  (popcount * 31, cap 1000)
//!   mc0_ctl_hi_bits — enabled error classes in high 32 bits (popcount * 31, cap 1000)
//!   mc0_ctl_total   — combined coverage: (lo/2 + hi/2).min(1000)
//!   mc0_ctl_ema     — EMA of mc0_ctl_total across samples (alpha = 1/8)
//!
//! Sample gate: every 2000 ticks (age % 2000 == 0).
//! MCA guard: CPUID leaf 1, EDX bit 14 must be set; if absent all signals zero.

#![allow(dead_code)]

use crate::sync::Mutex;

// MSR address for IA32_MC0_CTL (Machine Check Bank 0 Control)
const IA32_MC0_CTL: u32 = 0x400;

pub struct MsrMc0CtlState {
    /// Popcount of enabled error classes in the low 32 bits, scaled 0-1000.
    pub mc0_ctl_lo_bits: u16,
    /// Popcount of enabled error classes in the high 32 bits, scaled 0-1000.
    pub mc0_ctl_hi_bits: u16,
    /// Combined coverage index: (lo_bits/2 + hi_bits/2).min(1000).
    pub mc0_ctl_total: u16,
    /// Exponential moving average of mc0_ctl_total across samples.
    pub mc0_ctl_ema: u16,
    /// Internal tick counter (not used for gating, kept for diagnostics).
    pub tick_count: u32,
}

impl MsrMc0CtlState {
    pub const fn new() -> Self {
        Self {
            mc0_ctl_lo_bits: 0,
            mc0_ctl_hi_bits: 0,
            mc0_ctl_total: 0,
            mc0_ctl_ema: 0,
            tick_count: 0,
        }
    }
}

pub static MSR_MC0_CTL: Mutex<MsrMc0CtlState> = Mutex::new(MsrMc0CtlState::new());

// ---------------------------------------------------------------------------
// CPUID MCA feature check — CPUID leaf 1, EDX bit 14
// ---------------------------------------------------------------------------

/// Returns true if the CPU advertises MCA support via CPUID leaf 1 EDX bit 14.
/// Uses push rbx / cpuid / mov esi,edx / pop rbx per the MCA guard spec so
/// that rbx is preserved across the CPUID call regardless of register pressure.
#[inline]
fn mca_supported() -> bool {
    let edx_val: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov esi, edx",
            "pop rbx",
            in("eax")  1u32,
            out("esi") edx_val,
            // CPUID also writes eax/ecx/edx; declare as late outputs so LLVM
            // knows they are clobbered.  rbx is saved manually above.
            lateout("eax") _,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, preserves_flags),
        );
    }
    (edx_val >> 14) & 1 == 1
}

// ---------------------------------------------------------------------------
// MSR read — rdmsr 0x400
// ---------------------------------------------------------------------------

/// Read IA32_MC0_CTL (MSR 0x400). Returns (eax=lo, edx=hi).
/// SAFETY: Caller must confirm MCA is supported (CPUID) and that we are at
/// ring 0. Executing rdmsr in user-mode or against an unsupported MSR raises
/// a #GP; the MCA guard before every call site prevents that.
#[inline]
unsafe fn rdmsr_mc0_ctl() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx")  IA32_MC0_CTL,
        out("eax") lo,
        out("edx") hi,
        options(nostack, preserves_flags),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Software popcount — no floats, no std
// ---------------------------------------------------------------------------

/// Count set bits in a u32 using the classic parallel-prefix method.
#[inline]
fn popcount32(mut x: u32) -> u32 {
    // Kernighan's bit-trick unrolled via parallel prefix (no division, no float).
    x = x - ((x >> 1) & 0x5555_5555);
    x = (x & 0x3333_3333) + ((x >> 2) & 0x3333_3333);
    x = (x + (x >> 4)) & 0x0f0f_0f0f;
    x = x.wrapping_mul(0x0101_0101) >> 24;
    x
}

// ---------------------------------------------------------------------------
// Public tick
// ---------------------------------------------------------------------------

pub fn tick(age: u32) {
    let mut state = MSR_MC0_CTL.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample gate: only process on ticks where age is a multiple of 2000.
    if age % 2000 != 0 {
        return;
    }

    // MCA guard: check CPUID leaf 1 EDX bit 14 before touching the MSR.
    if !mca_supported() {
        serial_println!(
            "[msr_mc0_ctl] MCA not supported (CPUID leaf 1 EDX bit 14 = 0) — signals zeroed"
        );
        // Leave all signals at zero; do not attempt rdmsr.
        return;
    }

    // Read IA32_MC0_CTL (0x400).
    let (lo, hi) = unsafe { rdmsr_mc0_ctl() };

    // --- mc0_ctl_lo_bits ---
    // popcount(lo) in [0, 32]; scale by *31, cap at 1000.
    let lo_pop: u16 = popcount32(lo) as u16;                       // 0..=32
    let mc0_ctl_lo_bits: u16 = lo_pop.saturating_mul(31).min(1000);

    // --- mc0_ctl_hi_bits ---
    let hi_pop: u16 = popcount32(hi) as u16;                       // 0..=32
    let mc0_ctl_hi_bits: u16 = hi_pop.saturating_mul(31).min(1000);

    // --- mc0_ctl_total ---
    // (mc0_ctl_lo_bits / 2 + mc0_ctl_hi_bits / 2).min(1000)
    let mc0_ctl_total: u16 = (mc0_ctl_lo_bits / 2)
        .saturating_add(mc0_ctl_hi_bits / 2)
        .min(1000);

    // --- mc0_ctl_ema ---
    // EMA formula: (old * 7 + new_val) / 8, computed in u32 to prevent overflow,
    // then narrowed back to u16.
    let old_ema: u32 = state.mc0_ctl_ema as u32;
    let ema_u32: u32 = old_ema.wrapping_mul(7).wrapping_add(mc0_ctl_total as u32) / 8;
    let mc0_ctl_ema: u16 = ema_u32.min(1000) as u16;

    // Commit all signals.
    state.mc0_ctl_lo_bits = mc0_ctl_lo_bits;
    state.mc0_ctl_hi_bits = mc0_ctl_hi_bits;
    state.mc0_ctl_total   = mc0_ctl_total;
    state.mc0_ctl_ema     = mc0_ctl_ema;

    serial_println!(
        "[msr_mc0_ctl] age={} lo={:#010x}(pop={} sig={}) hi={:#010x}(pop={} sig={}) total={} ema={}",
        age,
        lo, lo_pop, mc0_ctl_lo_bits,
        hi, hi_pop, mc0_ctl_hi_bits,
        mc0_ctl_total,
        mc0_ctl_ema,
    );
}

// ---------------------------------------------------------------------------
// Accessors
// ---------------------------------------------------------------------------

pub fn get_lo_bits() -> u16 { MSR_MC0_CTL.lock().mc0_ctl_lo_bits }
pub fn get_hi_bits() -> u16 { MSR_MC0_CTL.lock().mc0_ctl_hi_bits }
pub fn get_total()   -> u16 { MSR_MC0_CTL.lock().mc0_ctl_total }
pub fn get_ema()     -> u16 { MSR_MC0_CTL.lock().mc0_ctl_ema }

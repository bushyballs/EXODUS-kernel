#![allow(dead_code)]

//! msr_ia32_aperf_sense — ANIMA life module
//!
//! Reads IA32_APERF (0xE8) and IA32_MPERF (0xE7) MSRs to derive ANIMA's
//! smoothed sense of how hard she is actually running relative to her maximum
//! sustainable rate.
//!
//! Hardware:
//!   IA32_APERF 0xE8 — actual performance counter; increments at actual execution rate
//!   IA32_MPERF 0xE7 — maximum performance counter; increments at max non-turbo rate
//!
//! Guard: CPUID leaf 6 EAX bit 0 (DTS — proxy for APERF/MPERF support)
//!
//! Signals (all u16, range 0–1000):
//!   aperf_delta — wrapping delta of APERF lo between ticks, scaled to 0-1000
//!   mperf_delta — wrapping delta of MPERF lo between ticks, scaled to 0-1000
//!   freq_ratio  — aperf_delta / mperf_delta * 1000; actual/max frequency ratio
//!   freq_ema    — EMA-8 of freq_ratio; ANIMA's smoothed sense of exertion
//!
//! Tick gate: every 500 ticks.

use core::arch::asm;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// MSR addresses
// ---------------------------------------------------------------------------

const IA32_MPERF: u32 = 0xE7;
const IA32_APERF: u32 = 0xE8;

// ---------------------------------------------------------------------------
// Tick gate
// ---------------------------------------------------------------------------

const TICK_GATE: u32 = 500;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct State {
    last_aperf: u32,
    last_mperf: u32,
    aperf_delta: u16,
    mperf_delta: u16,
    freq_ratio:  u16,
    freq_ema:    u16,
}

impl State {
    const fn new() -> Self {
        State {
            last_aperf:  0,
            last_mperf:  0,
            aperf_delta: 0,
            mperf_delta: 0,
            freq_ratio:  0,
            freq_ema:    0,
        }
    }
}

static MODULE: Mutex<State> = Mutex::new(State::new());

// ---------------------------------------------------------------------------
// CPUID guard — leaf 6 EAX bit 0 = APERF/MPERF support
// ---------------------------------------------------------------------------

fn has_aperf_mperf() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax_val & 1) != 0
}

// ---------------------------------------------------------------------------
// MSR read — returns (lo, hi)
// ---------------------------------------------------------------------------

#[inline]
unsafe fn rdmsr(addr: u32) -> (u32, u32) {
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

// ---------------------------------------------------------------------------
// Signal helpers — pure integer, no floats
// ---------------------------------------------------------------------------

/// Scale a raw u32 delta to 0-1000 using only the low 16 bits of range.
/// Formula: min(delta / 65535, 1) * 1000
/// Implemented as: if delta >= 65535 { 1000 } else { delta * 1000 / 65535 }
#[inline]
fn scale_delta(delta: u32) -> u16 {
    if delta >= 65535 {
        1000
    } else {
        (delta * 1000 / 65535) as u16
    }
}

/// EMA with weight 7/8 old, 1/8 new — wrapping_mul + saturating_add per spec.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new_val as u32) / 8) as u16
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Read initial APERF and MPERF counter values so the first delta is accurate.
pub fn init() {
    if !has_aperf_mperf() {
        crate::serial_println!(
            "[msr_ia32_aperf_sense] APERF/MPERF not supported (CPUID leaf 6 EAX bit 0 = 0) — signals zeroed"
        );
        return;
    }

    let (aperf_lo, _) = unsafe { rdmsr(IA32_APERF) };
    let (mperf_lo, _) = unsafe { rdmsr(IA32_MPERF) };

    let mut s = MODULE.lock();
    s.last_aperf  = aperf_lo;
    s.last_mperf  = mperf_lo;
    s.aperf_delta = 0;
    s.mperf_delta = 0;
    s.freq_ratio  = 0;
    s.freq_ema    = 0;

    crate::serial_println!(
        "[msr_ia32_aperf_sense] init OK — APERF=0x{:08x} MPERF=0x{:08x}",
        aperf_lo,
        mperf_lo,
    );
}

/// Update all signals. Gated to every 500 ticks.
pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_aperf_mperf() {
        return;
    }

    // Read current low 32 bits of each counter.
    let (aperf_lo, _) = unsafe { rdmsr(IA32_APERF) };
    let (mperf_lo, _) = unsafe { rdmsr(IA32_MPERF) };

    let mut s = MODULE.lock();

    // Wrapping delta of low 32-bit values between ticks.
    let aperf_raw_delta: u32 = aperf_lo.wrapping_sub(s.last_aperf);
    let mperf_raw_delta: u32 = mperf_lo.wrapping_sub(s.last_mperf);

    // Scale deltas: min(delta/65535, 1) * 1000
    let aperf_delta: u16 = scale_delta(aperf_raw_delta);
    let mperf_delta: u16 = scale_delta(mperf_raw_delta);

    // freq_ratio: actual/max frequency ratio (0-1000).
    // If mperf_delta == 0 the CPU ran at 0% of max — ratio is 0.
    let freq_ratio: u16 = if mperf_delta > 0 {
        ((aperf_delta as u32 * 1000) / mperf_delta as u32).min(1000) as u16
    } else {
        0
    };

    // EMA of freq_ratio — ANIMA's smoothed exertion sense.
    let freq_ema: u16 = ema(s.freq_ema, freq_ratio);

    // Commit.
    s.last_aperf  = aperf_lo;
    s.last_mperf  = mperf_lo;
    s.aperf_delta = aperf_delta;
    s.mperf_delta = mperf_delta;
    s.freq_ratio  = freq_ratio;
    s.freq_ema    = freq_ema;

    crate::serial_println!(
        "[msr_ia32_aperf_sense] age={} aperf_delta={} mperf_delta={} freq_ratio={} freq_ema={}",
        age,
        aperf_delta,
        mperf_delta,
        freq_ratio,
        freq_ema,
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

/// Delta of APERF low 32-bit counter between ticks, scaled to 0-1000.
pub fn get_aperf_delta() -> u16 {
    MODULE.lock().aperf_delta
}

/// Delta of MPERF low 32-bit counter between ticks, scaled to 0-1000.
pub fn get_mperf_delta() -> u16 {
    MODULE.lock().mperf_delta
}

/// Actual/max frequency ratio this tick (0-1000).
pub fn get_freq_ratio() -> u16 {
    MODULE.lock().freq_ratio
}

/// EMA-smoothed actual/max frequency ratio — ANIMA's sense of how hard she runs.
pub fn get_freq_ema() -> u16 {
    MODULE.lock().freq_ema
}

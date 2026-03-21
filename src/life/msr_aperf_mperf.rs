#![allow(dead_code)]

//! msr_aperf_mperf — ANIMA life module
//!
//! Reads IA32_MPERF (0xE7) and IA32_APERF (0xE8) MSRs to derive an
//! actual-vs-max CPU frequency ratio as a biological performance signal.
//!
//! Signals (all u16, range 0-1000):
//!   aperf_lo        — low 16 bits of APERF scaled to 0-1000
//!   mperf_lo        — low 16 bits of MPERF scaled to 0-1000
//!   freq_ratio      — (aperf_lo * 1000) / (mperf_lo + 1)
//!   aperf_mperf_ema — EMA-8 of freq_ratio

use core::arch::asm;
use crate::sync::Mutex;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct MsrAperfMperfState {
    aperf_lo:        u16,
    mperf_lo:        u16,
    freq_ratio:      u16,
    aperf_mperf_ema: u16,
}

impl MsrAperfMperfState {
    const fn new() -> Self {
        MsrAperfMperfState {
            aperf_lo:        0,
            mperf_lo:        0,
            freq_ratio:      0,
            aperf_mperf_ema: 0,
        }
    }
}

static STATE: Mutex<MsrAperfMperfState> = Mutex::new(MsrAperfMperfState::new());

// ---------------------------------------------------------------------------
// CPUID guard
// CPUID leaf 6 EAX bit 0 = APERFMPERF support
// ---------------------------------------------------------------------------

fn has_aperfmperf() -> bool {
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
// MSR read
// ---------------------------------------------------------------------------

/// Read a 64-bit MSR. Returns the low 32 bits (EAX) and high 32 bits (EDX).
#[inline]
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem),
    );
    (lo, hi)
}

// ---------------------------------------------------------------------------
// Helpers — pure integer, no floats
// ---------------------------------------------------------------------------

/// Scale raw u16 counter bits into the 0-1000 signal range.
/// Uses: (raw * 1000) / 65535, computed in u32 to avoid overflow.
#[inline]
fn scale_to_signal(raw: u16) -> u16 {
    let wide: u32 = (raw as u32) * 1000 / 65535;
    if wide > 1000 { 1000 } else { wide as u16 }
}

/// EMA with weight 7/8 on old value, 1/8 on new.
/// Computed in u32 to avoid overflow, result clamped to 1000.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let wide: u32 = ((old as u32) * 7 + (new_val as u32)) / 8;
    if wide > 1000 { 1000 } else { wide as u16 }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn init() {
    if !has_aperfmperf() {
        crate::serial_println!(
            "[msr_aperf_mperf] APERFMPERF not supported — signals zeroed"
        );
        return;
    }
    crate::serial_println!("[msr_aperf_mperf] init OK — APERFMPERF supported");
}

pub fn tick(age: u32) {
    // Sampling gate: every 500 ticks.
    if age % 500 != 0 {
        return;
    }

    // Hardware guard: skip silently if feature absent.
    if !has_aperfmperf() {
        return;
    }

    // Read MSRs — MPERF = 0xE7, APERF = 0xE8.
    let (mperf_lo_raw, _mperf_hi) = unsafe { rdmsr(0xE7) };
    let (aperf_lo_raw, _aperf_hi) = unsafe { rdmsr(0xE8) };

    // Take low 16 bits of each counter.
    let mperf_raw16: u16 = (mperf_lo_raw & 0xFFFF) as u16;
    let aperf_raw16: u16 = (aperf_lo_raw & 0xFFFF) as u16;

    // Scale to 0-1000 signals.
    let mperf_sig: u16 = scale_to_signal(mperf_raw16);
    let aperf_sig: u16 = scale_to_signal(aperf_raw16);

    // freq_ratio = (aperf_sig * 1000) / (mperf_sig + 1), clamped to 1000.
    let ratio_wide: u32 = (aperf_sig as u32) * 1000 / ((mperf_sig as u32) + 1);
    let freq_ratio: u16 = if ratio_wide > 1000 { 1000 } else { ratio_wide as u16 };

    // Update state under lock.
    let mut guard = STATE.lock();
    let new_ema = ema(guard.aperf_mperf_ema, freq_ratio);

    guard.aperf_lo        = aperf_sig;
    guard.mperf_lo        = mperf_sig;
    guard.freq_ratio      = freq_ratio;
    guard.aperf_mperf_ema = new_ema;

    crate::serial_println!(
        "[msr_aperf_mperf] age={} aperf={} mperf={} ratio={} ema={}",
        age,
        aperf_sig,
        mperf_sig,
        freq_ratio,
        new_ema,
    );
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_aperf_lo() -> u16 {
    STATE.lock().aperf_lo
}

pub fn get_mperf_lo() -> u16 {
    STATE.lock().mperf_lo
}

pub fn get_freq_ratio() -> u16 {
    STATE.lock().freq_ratio
}

pub fn get_aperf_mperf_ema() -> u16 {
    STATE.lock().aperf_mperf_ema
}

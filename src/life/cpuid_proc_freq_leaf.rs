#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct CpuidProcFreqState {
    base_freq:      u16, // EAX / 4, capped at 1000
    max_freq:       u16, // EBX / 4, capped at 1000
    ref_freq:       u16, // ECX * 10, capped at 1000
    freq_ratio_ema: u16, // EMA of (max_freq - base_freq).min(1000)
}

impl CpuidProcFreqState {
    const fn zero() -> Self {
        Self {
            base_freq:      0,
            max_freq:       0,
            ref_freq:       0,
            freq_ratio_ema: 0,
        }
    }
}

static STATE: Mutex<CpuidProcFreqState> = Mutex::new(CpuidProcFreqState::zero());

// ── CPUID helpers ─────────────────────────────────────────────────────────────

/// Returns true when CPUID leaf 0x16 is present (max leaf >= 0x16).
fn has_leaf16() -> bool {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    max_leaf >= 0x16
}

/// Read CPUID leaf 0x16, sub-leaf 0.
/// Returns (eax, ebx, ecx) = (base_MHz, max_MHz, ref_MHz).
fn read_leaf16() -> (u32, u32, u32) {
    let eax_val: u32;
    let ebx_val: u32;
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov esi, ebx",
            "pop rbx",
            inout("eax") 0x16u32 => eax_val,
            in("ecx")  0u32,
            out("esi") ebx_val,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax_val, ebx_val, ecx_val)
}

// ── Signal mapping ────────────────────────────────────────────────────────────

/// base_freq: EAX / 4, capped at 1000.
/// 4000 MHz → 1000; 2400 MHz → 600.
#[inline]
fn map_base(eax: u32) -> u16 {
    let v = eax / 4;
    if v > 1000 { 1000 } else { v as u16 }
}

/// max_freq: EBX / 4, capped at 1000.
#[inline]
fn map_max(ebx: u32) -> u16 {
    let v = ebx / 4;
    if v > 1000 { 1000 } else { v as u16 }
}

/// ref_freq: ECX * 10, capped at 1000.
/// 100 MHz → 1000; 50 MHz → 500.
#[inline]
fn map_ref(ecx: u32) -> u16 {
    let v = ecx * 10;
    if v > 1000 { 1000 } else { v as u16 }
}

/// EMA: (old * 7 + new_val) / 8, computed in u32, cast to u16.
#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32) * 7 + (new_val as u32)) / 8) as u16
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_leaf16() {
        crate::serial_println!(
            "[cpuid_proc_freq_leaf] CPUID leaf 0x16 not available — signals zeroed"
        );
        return;
    }

    let (eax, ebx, ecx) = read_leaf16();

    let base = map_base(eax);
    let max  = map_max(ebx);
    let reff = map_ref(ecx);

    let headroom = {
        let h = (max as u32).saturating_sub(base as u32);
        if h > 1000 { 1000u16 } else { h as u16 }
    };

    let mut s = STATE.lock();
    s.base_freq      = base;
    s.max_freq       = max;
    s.ref_freq       = reff;
    s.freq_ratio_ema = headroom; // seed EMA with first reading

    crate::serial_println!(
        "[cpuid_proc_freq_leaf] init: base={} max={} ref={} ema={}",
        s.base_freq, s.max_freq, s.ref_freq, s.freq_ratio_ema
    );
}

pub fn tick(age: u32) {
    // Sample every 8000 ticks — CPU frequency info is essentially static.
    if age % 8000 != 0 {
        return;
    }

    if !has_leaf16() {
        return;
    }

    let (eax, ebx, ecx) = read_leaf16();

    let base = map_base(eax);
    let max  = map_max(ebx);
    let reff = map_ref(ecx);

    let headroom = {
        let h = (max as u32).saturating_sub(base as u32);
        if h > 1000 { 1000u16 } else { h as u16 }
    };

    let mut s = STATE.lock();
    s.base_freq      = base;
    s.max_freq       = max;
    s.ref_freq       = reff;
    s.freq_ratio_ema = ema(s.freq_ratio_ema, headroom);

    crate::serial_println!(
        "[cpuid_proc_freq_leaf] age={} base={} max={} ref={} ema={}",
        age, s.base_freq, s.max_freq, s.ref_freq, s.freq_ratio_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_base_freq() -> u16 {
    STATE.lock().base_freq
}

pub fn get_max_freq() -> u16 {
    STATE.lock().max_freq
}

pub fn get_ref_freq() -> u16 {
    STATE.lock().ref_freq
}

pub fn get_freq_ratio_ema() -> u16 {
    STATE.lock().freq_ratio_ema
}

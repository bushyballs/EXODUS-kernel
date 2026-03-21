#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct MsrIa32PmEnableState {
    hwp_enabled:    u16,
    pm_lo_sense:    u16,
    pm_hi_sense:    u16,
    pm_enable_ema:  u16,
}

impl MsrIa32PmEnableState {
    const fn new() -> Self {
        Self {
            hwp_enabled:   0,
            pm_lo_sense:   0,
            pm_hi_sense:   0,
            pm_enable_ema: 0,
        }
    }
}

static STATE: Mutex<MsrIa32PmEnableState> = Mutex::new(MsrIa32PmEnableState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

/// Returns true if CPUID leaf 6 EAX bit 7 indicates HWP support.
fn has_hwp() -> bool {
    let eax_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax_val,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (eax_val >> 7) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read IA32_PM_ENABLE (MSR 0x770).
/// Returns (lo_32, hi_32).
unsafe fn read_msr_770() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") 0x770u32,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (lo, hi)
}

// ── EMA helper ───────────────────────────────────────────────────────────────

#[inline(always)]
fn ema(old: u16, new_val: u16) -> u16 {
    let result: u32 = (old as u32 * 7 + new_val as u32) / 8;
    result as u16
}

// ── Signal derivation ────────────────────────────────────────────────────────

/// Derive the four signals from the raw MSR lo-dword.
fn derive_signals(lo: u32, prev_ema: u16) -> (u16, u16, u16, u16) {
    // hwp_enabled: bit 0 → 0 or 1000
    let hwp_enabled: u16 = if lo & 1 != 0 { 1000 } else { 0 };

    // pm_lo_sense: bits [7:1] (reserved / platform-specific), scaled × 8, clamped 1000
    let lo_bits = (lo >> 1) & 0x7F;
    let pm_lo_sense: u16 = ((lo_bits * 8).min(1000)) as u16;

    // pm_hi_sense: bits [31:8] activity, scaled × 4, clamped 1000
    let hi_bits = (lo >> 8) & 0xFF;
    let pm_hi_sense: u16 = ((hi_bits * 4).min(1000)) as u16;

    // pm_enable_ema: EMA of hwp_enabled
    let pm_enable_ema: u16 = ema(prev_ema, hwp_enabled);

    (hwp_enabled, pm_lo_sense, pm_hi_sense, pm_enable_ema)
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.hwp_enabled   = 0;
    s.pm_lo_sense   = 0;
    s.pm_hi_sense   = 0;
    s.pm_enable_ema = 0;
    crate::serial_println!(
        "[msr_ia32_pm_enable] init: hwp_supported={}",
        has_hwp()
    );
}

pub fn tick(age: u32) {
    // Sampling gate: every 6000 ticks
    if age % 6000 != 0 {
        return;
    }

    // CPUID guard — HWP must be supported
    if !has_hwp() {
        return;
    }

    let (lo, _hi) = unsafe { read_msr_770() };

    let prev_ema = {
        let s = STATE.lock();
        s.pm_enable_ema
    };

    let (hwp_enabled, pm_lo_sense, pm_hi_sense, pm_enable_ema) =
        derive_signals(lo, prev_ema);

    {
        let mut s = STATE.lock();
        s.hwp_enabled   = hwp_enabled;
        s.pm_lo_sense   = pm_lo_sense;
        s.pm_hi_sense   = pm_hi_sense;
        s.pm_enable_ema = pm_enable_ema;
    }

    crate::serial_println!(
        "[msr_ia32_pm_enable] age={} hwp_en={} lo={} hi={} ema={}",
        age,
        hwp_enabled,
        pm_lo_sense,
        pm_hi_sense,
        pm_enable_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_hwp_enabled() -> u16 {
    STATE.lock().hwp_enabled
}

pub fn get_pm_lo_sense() -> u16 {
    STATE.lock().pm_lo_sense
}

pub fn get_pm_hi_sense() -> u16 {
    STATE.lock().pm_hi_sense
}

pub fn get_pm_enable_ema() -> u16 {
    STATE.lock().pm_enable_ema
}

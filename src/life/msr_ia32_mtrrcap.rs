#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct MtrrCapState {
    mtrrcap_vcnt: u16,
    mtrrcap_fix:  u16,
    mtrrcap_wc:   u16,
    mtrrcap_ema:  u16,
}

static STATE: Mutex<MtrrCapState> = Mutex::new(MtrrCapState {
    mtrrcap_vcnt: 0,
    mtrrcap_fix:  0,
    mtrrcap_wc:   0,
    mtrrcap_ema:  0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_mtrr() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _, lateout("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 12) & 1 != 0
}

// ── MSR read ─────────────────────────────────────────────────────────────────

/// Read a 64-bit MSR. Returns (lo, hi).
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

// ── Signal computation ───────────────────────────────────────────────────────

fn compute_signals(lo: u32) -> (u16, u16, u16) {
    // bits [7:0] = VCNT
    let vcnt_raw = (lo & 0xFF) as u32;
    // VCNT × 100, capped at 1000
    let vcnt_sig: u16 = {
        let v = vcnt_raw * 100;
        if v > 1000 { 1000 } else { v as u16 }
    };

    // bit 8 = FIX
    let fix_sig: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };

    // bit 10 = WC
    let wc_sig: u16 = if (lo >> 10) & 1 != 0 { 1000 } else { 0 };

    (vcnt_sig, fix_sig, wc_sig)
}

fn compute_composite(vcnt: u16, fix: u16, wc: u16) -> u16 {
    // vcnt/2 + fix/4 + wc/4
    let v = (vcnt as u32) / 2 + (fix as u32) / 4 + (wc as u32) / 4;
    if v > 1000 { 1000 } else { v as u16 }
}

fn ema(old: u16, new_val: u16) -> u16 {
    let result = ((old as u32) * 7 + (new_val as u32)) / 8;
    result as u16
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_mtrr() {
        crate::serial_println!(
            "[msr_ia32_mtrrcap] MTRR not supported on this CPU — module disabled"
        );
        return;
    }

    let lo = unsafe { rdmsr(0xFE).0 };
    let (vcnt_sig, fix_sig, wc_sig) = compute_signals(lo);
    let composite = compute_composite(vcnt_sig, fix_sig, wc_sig);

    let mut s = STATE.lock();
    s.mtrrcap_vcnt = vcnt_sig;
    s.mtrrcap_fix  = fix_sig;
    s.mtrrcap_wc   = wc_sig;
    s.mtrrcap_ema  = composite; // seed EMA with first reading

    crate::serial_println!(
        "[msr_ia32_mtrrcap] init: vcnt={} fix={} wc={} ema={}",
        s.mtrrcap_vcnt, s.mtrrcap_fix, s.mtrrcap_wc, s.mtrrcap_ema
    );
}

pub fn tick(age: u32) {
    // MTRR capability is static hardware info — sample every 10 000 ticks
    if age % 10_000 != 0 {
        return;
    }

    if !has_mtrr() {
        return;
    }

    let lo = unsafe { rdmsr(0xFE).0 };
    let (vcnt_sig, fix_sig, wc_sig) = compute_signals(lo);
    let composite = compute_composite(vcnt_sig, fix_sig, wc_sig);

    let mut s = STATE.lock();
    s.mtrrcap_vcnt = vcnt_sig;
    s.mtrrcap_fix  = fix_sig;
    s.mtrrcap_wc   = wc_sig;
    s.mtrrcap_ema  = ema(s.mtrrcap_ema, composite);

    crate::serial_println!(
        "[msr_ia32_mtrrcap] age={} vcnt={} fix={} wc={} ema={}",
        age, s.mtrrcap_vcnt, s.mtrrcap_fix, s.mtrrcap_wc, s.mtrrcap_ema
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_mtrrcap_vcnt() -> u16 {
    STATE.lock().mtrrcap_vcnt
}

pub fn get_mtrrcap_fix() -> u16 {
    STATE.lock().mtrrcap_fix
}

pub fn get_mtrrcap_wc() -> u16 {
    STATE.lock().mtrrcap_wc
}

pub fn get_mtrrcap_ema() -> u16 {
    STATE.lock().mtrrcap_ema
}

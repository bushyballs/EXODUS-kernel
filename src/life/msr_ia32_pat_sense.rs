#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_PAT: u32 = 0x277;

pub struct State {
    pat_wb_count:   u16,
    pat_uc_count:   u16,
    pat_wc_present: u16,
    pat_ema:        u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    pat_wb_count:   0,
    pat_uc_count:   0,
    pat_wc_present: 0,
    pat_ema:        0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_pat() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 16) & 1 == 1
}

// ── MSR read ─────────────────────────────────────────────────────────────────

fn read_pat() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_PAT,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn popcount(mut v: u32) -> u32 {
    let mut count: u32 = 0;
    while v != 0 {
        count += v & 1;
        v >>= 1;
    }
    count
}

/// Extract the 3-bit memory type from a PAT register half.
/// n = 0..3; shifts by n*8 then masks lowest 3 bits.
fn extract_pa(half: u32, n: u32) -> u32 {
    (half >> (n * 8)) & 0x7
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("[msr_ia32_pat_sense] init: PAT present={}", has_pat());
}

pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }
    if !has_pat() {
        return;
    }

    let (lo, hi) = read_pat();

    let mut wb_count: u32 = 0; // PA value == 6 (WB)
    let mut uc_count: u32 = 0; // PA value == 0 (UC)
    let mut wc_any:   bool = false; // any PA value == 1 (WC)

    for n in 0u32..4 {
        let pa_lo = extract_pa(lo, n);
        if pa_lo == 6 { wb_count += 1; }
        if pa_lo == 0 { uc_count += 1; }
        if pa_lo == 1 { wc_any = true; }

        let pa_hi = extract_pa(hi, n);
        if pa_hi == 6 { wb_count += 1; }
        if pa_hi == 0 { uc_count += 1; }
        if pa_hi == 1 { wc_any = true; }
    }

    // Scale: (count * 1000 / 8).min(1000)
    let wb_sig  = ((wb_count * 1000 / 8) as u16).min(1000);
    let uc_sig  = ((uc_count * 1000 / 8) as u16).min(1000);
    let wc_sig  = if wc_any { 1000u16 } else { 0u16 };

    let mut state = MODULE.lock();
    state.pat_ema        = ema(state.pat_ema, wb_sig);
    state.pat_wb_count   = wb_sig;
    state.pat_uc_count   = uc_sig;
    state.pat_wc_present = wc_sig;

    serial_println!(
        "[msr_ia32_pat_sense] age={} wb={} uc={} wc={} ema={}",
        age, wb_sig, uc_sig, wc_sig, state.pat_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pat_wb_count() -> u16 {
    MODULE.lock().pat_wb_count
}

pub fn get_pat_uc_count() -> u16 {
    MODULE.lock().pat_uc_count
}

pub fn get_pat_wc_present() -> u16 {
    MODULE.lock().pat_wc_present
}

pub fn get_pat_ema() -> u16 {
    MODULE.lock().pat_ema
}

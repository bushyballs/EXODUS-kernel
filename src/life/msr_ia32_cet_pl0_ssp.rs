#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ────────────────────────────────────────────────────────────────────

struct CetPl0SspState {
    pl0_ssp_nonzero: u16,
    pl3_ssp_nonzero: u16,
    ssp_both_active: u16,
    ssp_ema:         u16,
}

impl CetPl0SspState {
    const fn new() -> Self {
        Self {
            pl0_ssp_nonzero: 0,
            pl3_ssp_nonzero: 0,
            ssp_both_active: 0,
            ssp_ema:         0,
        }
    }
}

static STATE: Mutex<CetPl0SspState> = Mutex::new(CetPl0SspState::new());

// ── CPUID guard ──────────────────────────────────────────────────────────────

fn has_cet() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 7u32 => _,
            in("ecx") 0u32,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx_val >> 7) & 1 != 0
}

// ── RDMSR helper ─────────────────────────────────────────────────────────────

/// Read a 64-bit MSR; returns (edx, eax) — caller uses eax (low 32 bits).
unsafe fn rdmsr(msr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
        options(nostack, nomem)
    );
    (hi, lo)
}

// ── Public API ───────────────────────────────────────────────────────────────

pub fn init() {
    if !has_cet() {
        crate::serial_println!(
            "[msr_ia32_cet_pl0_ssp] CET-SS not supported by CPU — module inactive"
        );
        return;
    }
    crate::serial_println!(
        "[msr_ia32_cet_pl0_ssp] init — CET-SS supported, monitoring MSR 0x6A4 / 0x6A7"
    );
}

pub fn tick(age: u32) {
    // Sample every 1000 ticks.
    if age % 1000 != 0 {
        return;
    }

    if !has_cet() {
        return;
    }

    // Read IA32_PL0_SSP (0x6A4) and IA32_PL3_SSP (0x6A7).
    let pl0_ssp_lo: u32 = unsafe { rdmsr(0x6A4).1 };
    let pl3_ssp_lo: u32 = unsafe { rdmsr(0x6A7).1 };

    // Derive signals (all in 0–1000 range).
    let pl0_nonzero: u16 = if pl0_ssp_lo != 0 { 1000 } else { 0 };
    let pl3_nonzero: u16 = if pl3_ssp_lo != 0 { 1000 } else { 0 };
    let both_active: u16 = if pl0_ssp_lo != 0 && pl3_ssp_lo != 0 { 1000 } else { 0 };

    // EMA input: pl0_nonzero/4 + pl3_nonzero/4 + both_active/2
    // All arithmetic in u32 to avoid overflow before dividing.
    let composite: u16 = (
        (pl0_nonzero as u32 / 4)
        + (pl3_nonzero as u32 / 4)
        + (both_active as u32 / 2)
    ) as u16;

    let mut state = STATE.lock();

    // EMA: (old * 7 + new_val) / 8 in u32, then cast to u16.
    let new_ema: u16 = ((state.ssp_ema as u32 * 7 + composite as u32) / 8) as u16;

    state.pl0_ssp_nonzero = pl0_nonzero;
    state.pl3_ssp_nonzero = pl3_nonzero;
    state.ssp_both_active = both_active;
    state.ssp_ema         = new_ema;

    crate::serial_println!(
        "[msr_ia32_cet_pl0_ssp] age={} pl0={} pl3={} both={} ema={}",
        age,
        pl0_nonzero,
        pl3_nonzero,
        both_active,
        new_ema,
    );
}

// ── Getters ──────────────────────────────────────────────────────────────────

pub fn get_pl0_ssp_nonzero() -> u16 {
    STATE.lock().pl0_ssp_nonzero
}

pub fn get_pl3_ssp_nonzero() -> u16 {
    STATE.lock().pl3_ssp_nonzero
}

pub fn get_ssp_both_active() -> u16 {
    STATE.lock().ssp_both_active
}

pub fn get_ssp_ema() -> u16 {
    STATE.lock().ssp_ema
}

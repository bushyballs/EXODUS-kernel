#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct PqrAssocState {
    rmid_sense:   u16,
    clos_sense:   u16,
    rmid_nonzero: u16,
    pqr_ema:      u16,
}

static STATE: Mutex<PqrAssocState> = Mutex::new(PqrAssocState {
    rmid_sense:   0,
    clos_sense:   0,
    rmid_nonzero: 0,
    pqr_ema:      0,
});

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_rdt() -> bool {
    let max_leaf: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0x0F {
        return false;
    }
    let edx_0f: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Fu32 => _,
            in("ecx") 0u32,
            lateout("ecx") _,
            lateout("edx") edx_0f,
            options(nostack, nomem)
        );
    }
    (edx_0f >> 1) & 1 != 0
}

// ── RDMSR ─────────────────────────────────────────────────────────────────────

/// Returns (lo, hi) for the requested MSR address.
#[inline]
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
    (lo, hi)
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = STATE.lock();
    s.rmid_sense   = 0;
    s.clos_sense   = 0;
    s.rmid_nonzero = 0;
    s.pqr_ema      = 0;
    crate::serial_println!("[msr_ia32_pqr_assoc] init — IA32_PQR_ASSOC (0xC8D) module ready");
}

pub fn tick(age: u32) {
    // Sample every 1000 ticks.
    if age % 1000 != 0 {
        return;
    }

    // CPUID guard — RDT monitoring must be available.
    if !has_rdt() {
        return;
    }

    // Read IA32_PQR_ASSOC MSR 0xC8D.
    let (lo, hi) = unsafe { rdmsr(0xC8D) };

    // rmid_sense: bits[9:0] of lo, scaled 0–1000.
    // Multiply by 1000 then divide by 1024 (capacity of a 10-bit field).
    let rmid_raw = lo & 0x3FF;                                  // 0–1023
    let rmid_sense_u32 = (rmid_raw as u32) * 1000 / 1024;
    let rmid_sense = if rmid_sense_u32 > 1000 { 1000u16 } else { rmid_sense_u32 as u16 };

    // clos_sense: bits[1:0] of hi (== bits[33:32] of the 64-bit MSR), scaled 0–1000.
    // 0→0, 1→333, 2→666, 3→999 — multiply by 333, cap at 1000.
    let clos_raw = hi & 0x3;                                    // 0–3
    let clos_sense_u32 = (clos_raw as u32) * 333;
    let clos_sense = if clos_sense_u32 > 1000 { 1000u16 } else { clos_sense_u32 as u16 };

    // rmid_nonzero: is RMID != 0?
    let rmid_nonzero: u16 = if rmid_raw != 0 { 1000 } else { 0 };

    // pqr_ema: composite = rmid_sense/2 + clos_sense/4 + rmid_nonzero/4
    // All arithmetic in u32 to avoid overflow before the EMA step.
    let composite_u32 = (rmid_sense as u32) / 2
        + (clos_sense as u32) / 4
        + (rmid_nonzero as u32) / 4;
    let composite = if composite_u32 > 1000 { 1000u16 } else { composite_u32 as u16 };

    let mut s = STATE.lock();

    // EMA: (old * 7 + new_val) / 8 — computed in u32, cast to u16.
    let ema_u32 = ((s.pqr_ema as u32) * 7 + (composite as u32)) / 8;
    let pqr_ema = if ema_u32 > 1000 { 1000u16 } else { ema_u32 as u16 };

    s.rmid_sense   = rmid_sense;
    s.clos_sense   = clos_sense;
    s.rmid_nonzero = rmid_nonzero;
    s.pqr_ema      = pqr_ema;

    crate::serial_println!(
        "[msr_ia32_pqr_assoc] age={} rmid={} clos={} nonzero={} ema={}",
        age, rmid_sense, clos_sense, rmid_nonzero, pqr_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_rmid_sense() -> u16 {
    STATE.lock().rmid_sense
}

pub fn get_clos_sense() -> u16 {
    STATE.lock().clos_sense
}

pub fn get_rmid_nonzero() -> u16 {
    STATE.lock().rmid_nonzero
}

pub fn get_pqr_ema() -> u16 {
    STATE.lock().pqr_ema
}

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── State ─────────────────────────────────────────────────────────────────────

struct Pmc4SenseState {
    pmc4_delta: u16,
    pmc5_delta: u16,
    pmc4_ema:   u16,
    pmc5_ema:   u16,
    last_c4:    u32,
    last_c5:    u32,
}

impl Pmc4SenseState {
    const fn new() -> Self {
        Self {
            pmc4_delta: 0,
            pmc5_delta: 0,
            pmc4_ema:   0,
            pmc5_ema:   0,
            last_c4:    0,
            last_c5:    0,
        }
    }
}

static STATE: Mutex<Pmc4SenseState> = Mutex::new(Pmc4SenseState::new());

// ── CPUID guard ───────────────────────────────────────────────────────────────

fn has_pmc4() -> bool {
    let ecx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") ecx_val,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // PDCM: leaf 1, ECX bit 15
    if (ecx_val >> 15) & 1 == 0 {
        return false;
    }
    let eax_0a: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x0Au32 => eax_0a,
            lateout("ecx") _,
            lateout("edx") _,
            options(nostack, nomem)
        );
    }
    // Number of GP counters per logical processor: leaf 0x0A, EAX bits[15:8]
    ((eax_0a >> 8) & 0xFF) >= 5
}

// ── MSR helpers ───────────────────────────────────────────────────────────────

/// Read a 64-bit MSR; returns the low 32 bits (sufficient for delta math).
#[inline]
unsafe fn rdmsr32(msr: u32) -> u32 {
    let lo: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        lateout("edx") _,
        options(nostack, nomem)
    );
    lo
}

// ── Mapping ───────────────────────────────────────────────────────────────────

/// Map a raw u16 delta (0–0xFFFF) to the 0–1000 signal range.
/// Uses integer arithmetic: `(delta as u32 * 1000) / 0xFFFF`, clamped to 1000.
#[inline]
fn map_to_signal(delta_u16: u16) -> u16 {
    let mapped = (delta_u16 as u32 * 1000) / 0xFFFF;
    if mapped > 1000 { 1000u16 } else { mapped as u16 }
}

// ── EMA ───────────────────────────────────────────────────────────────────────

#[inline]
fn ema(old: u16, new_val: u16) -> u16 {
    let result = (old as u32 * 7 + new_val as u32) / 8;
    if result > 1000 { 1000u16 } else { result as u16 }
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_pmc4() {
        crate::serial_println!("[msr_ia32_pmc4_sense] PDCM or PMC4/5 not supported — module disabled");
        return;
    }
    let mut s = STATE.lock();
    unsafe {
        s.last_c4 = rdmsr32(0xC5);
        s.last_c5 = rdmsr32(0xC6);
    }
    crate::serial_println!("[msr_ia32_pmc4_sense] init: seeded last_c4={} last_c5={}", s.last_c4, s.last_c5);
}

pub fn tick(age: u32) {
    // Sample every 300 ticks
    if age % 300 != 0 {
        return;
    }
    if !has_pmc4() {
        return;
    }

    let mut s = STATE.lock();

    let cur_c4: u32 = unsafe { rdmsr32(0xC5) };
    let cur_c5: u32 = unsafe { rdmsr32(0xC6) };

    // Delta: wrapping subtraction on low 32 bits, then take low 16 bits
    let raw4 = cur_c4.wrapping_sub(s.last_c4) as u16;
    let raw5 = cur_c5.wrapping_sub(s.last_c5) as u16;

    s.last_c4 = cur_c4;
    s.last_c5 = cur_c5;

    let sig4 = map_to_signal(raw4);
    let sig5 = map_to_signal(raw5);

    s.pmc4_delta = sig4;
    s.pmc5_delta = sig5;
    s.pmc4_ema   = ema(s.pmc4_ema, sig4);
    s.pmc5_ema   = ema(s.pmc5_ema, sig5);

    crate::serial_println!(
        "[msr_ia32_pmc4_sense] age={} pmc4={} pmc5={} ema4={} ema5={}",
        age, s.pmc4_delta, s.pmc5_delta, s.pmc4_ema, s.pmc5_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pmc4_delta() -> u16 {
    STATE.lock().pmc4_delta
}

pub fn get_pmc5_delta() -> u16 {
    STATE.lock().pmc5_delta
}

pub fn get_pmc4_ema() -> u16 {
    STATE.lock().pmc4_ema
}

pub fn get_pmc5_ema() -> u16 {
    STATE.lock().pmc5_ema
}

#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;

// ── CPUID guard ────────────────────────────────────────────────────────────────

fn has_pmc2() -> bool {
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
    ((eax_0a >> 8) & 0xFF) >= 3
}

// ── MSR read helper ────────────────────────────────────────────────────────────

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        lateout("eax") lo,
        lateout("edx") hi,
        options(nostack, nomem)
    );
    ((hi as u64) << 32) | (lo as u64)
}

// ── MSR addresses ──────────────────────────────────────────────────────────────

const IA32_PMC2: u32 = 0xC3;
const IA32_PMC3: u32 = 0xC4;

// ── Sampling interval ──────────────────────────────────────────────────────────

const SAMPLE_EVERY: u32 = 300;

// ── Signal mapping: clamp delta to 0-1000 ─────────────────────────────────────
//
// Raw PMC deltas can be very large; we map them to [0, 1000] by capping at a
// saturation constant chosen to keep the arithmetic in u32 without overflow.
// SAT = 2_000_000 means "2 M events per 300-tick window → signal 1000".

const SAT: u32 = 2_000_000;

fn delta_to_signal(delta: u32) -> u16 {
    // signal = min(delta, SAT) * 1000 / SAT   (all u32, no floats)
    let clamped = if delta > SAT { SAT } else { delta };
    (clamped * 1000 / SAT) as u16
}

// ── EMA: (old * 7 + new_val) / 8 in u32, result cast to u16 ──────────────────

fn ema(old: u16, new_val: u16) -> u16 {
    (((old as u32) * 7 + (new_val as u32)) / 8) as u16
}

// ── State ─────────────────────────────────────────────────────────────────────

struct Pmc2State {
    pmc2_delta: u16,
    pmc3_delta: u16,
    pmc2_ema:   u16,
    pmc3_ema:   u16,
    last_c2:    u32,
    last_c3:    u32,
}

impl Pmc2State {
    const fn zero() -> Self {
        Self {
            pmc2_delta: 0,
            pmc3_delta: 0,
            pmc2_ema:   0,
            pmc3_ema:   0,
            last_c2:    0,
            last_c3:    0,
        }
    }
}

static STATE: Mutex<Pmc2State> = Mutex::new(Pmc2State::zero());

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    if !has_pmc2() {
        crate::serial_println!("[msr_ia32_pmc2_sense] CPUID: PMC2 not available — module inactive");
        return;
    }
    let c2 = unsafe { rdmsr(IA32_PMC2) } as u32;
    let c3 = unsafe { rdmsr(IA32_PMC3) } as u32;
    let mut s = STATE.lock();
    s.last_c2 = c2;
    s.last_c3 = c3;
    crate::serial_println!(
        "[msr_ia32_pmc2_sense] init: seeded last_c2={} last_c3={}",
        c2, c3
    );
}

pub fn tick(age: u32) {
    if age % SAMPLE_EVERY != 0 {
        return;
    }
    if !has_pmc2() {
        return;
    }

    let c2_raw = unsafe { rdmsr(IA32_PMC2) } as u32;
    let c3_raw = unsafe { rdmsr(IA32_PMC3) } as u32;

    let mut s = STATE.lock();

    // Compute deltas (saturating on counter wrap)
    let d2 = c2_raw.wrapping_sub(s.last_c2);
    let d3 = c3_raw.wrapping_sub(s.last_c3);

    s.last_c2 = c2_raw;
    s.last_c3 = c3_raw;

    let sig2 = delta_to_signal(d2);
    let sig3 = delta_to_signal(d3);

    s.pmc2_delta = sig2;
    s.pmc3_delta = sig3;
    s.pmc2_ema   = ema(s.pmc2_ema, sig2);
    s.pmc3_ema   = ema(s.pmc3_ema, sig3);

    crate::serial_println!(
        "[msr_ia32_pmc2_sense] age={} pmc2={} pmc3={} ema2={} ema3={}",
        age, s.pmc2_delta, s.pmc3_delta, s.pmc2_ema, s.pmc3_ema
    );
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn get_pmc2_delta() -> u16 {
    STATE.lock().pmc2_delta
}

pub fn get_pmc3_delta() -> u16 {
    STATE.lock().pmc3_delta
}

pub fn get_pmc2_ema() -> u16 {
    STATE.lock().pmc2_ema
}

pub fn get_pmc3_ema() -> u16 {
    STATE.lock().pmc3_ema
}

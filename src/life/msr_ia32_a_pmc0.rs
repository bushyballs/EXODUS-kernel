#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_A_PMC0: u32 = 0x4C1;
const TICK_GATE: u32 = 400;

pub struct State {
    apmc0_lo:     u16,
    apmc0_delta:  u16,
    apmc0_active: u16,
    apmc0_ema:    u16,
    last_lo:      u32,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    apmc0_lo:     0,
    apmc0_delta:  0,
    apmc0_active: 0,
    apmc0_ema:    0,
    last_lo:      0,
});

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 15) & 1 == 1
}

fn perf_version() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    if max_leaf < 0xA {
        return 0;
    }
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax & 0xFF
}

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if !has_pdcm() || perf_version() < 2 {
        serial_println!("[msr_ia32_a_pmc0] PDCM or perf version < 2 — skipping init");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_A_PMC0);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    serial_println!("[msr_ia32_a_pmc0] init: last_lo={}", lo);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_pdcm() || perf_version() < 2 {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_A_PMC0);

    let mut s = MODULE.lock();

    // apmc0_lo: (lo & 0xFFFF) * 1000 / 65535, min 1000
    let lo_low = lo & 0xFFFF;
    let apmc0_lo = ((lo_low as u32 * 1000) / 65535).min(1000) as u16;

    // apmc0_delta: wrapping delta of lo since last tick
    let delta = lo.wrapping_sub(s.last_lo);
    let apmc0_delta = if delta >= 65536 {
        1000u16
    } else {
        ((delta as u32 * 1000) / 65536).min(1000) as u16
    };

    // apmc0_active: delta > 0 → 1000, else 0
    let apmc0_active: u16 = if delta > 0 { 1000 } else { 0 };

    // apmc0_ema: EMA of apmc0_delta
    let apmc0_ema = ema(s.apmc0_ema, apmc0_delta);

    s.last_lo     = lo;
    s.apmc0_lo    = apmc0_lo;
    s.apmc0_delta = apmc0_delta;
    s.apmc0_active = apmc0_active;
    s.apmc0_ema   = apmc0_ema;

    serial_println!(
        "[msr_ia32_a_pmc0] tick={} lo={} delta={} active={} ema={}",
        age, apmc0_lo, apmc0_delta, apmc0_active, apmc0_ema
    );
}

pub fn get_apmc0_lo() -> u16 {
    MODULE.lock().apmc0_lo
}

pub fn get_apmc0_delta() -> u16 {
    MODULE.lock().apmc0_delta
}

pub fn get_apmc0_active() -> u16 {
    MODULE.lock().apmc0_active
}

pub fn get_apmc0_ema() -> u16 {
    MODULE.lock().apmc0_ema
}

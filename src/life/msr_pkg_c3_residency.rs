#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_PKG_C3_RESIDENCY: u32 = 0x60E;
const TICK_GATE: u32 = 4500;

pub struct State {
    last_lo: u32,
    pkg_c3_lo: u16,
    pkg_c3_delta: u16,
    pkg_c3_active: u16,
    pkg_c3_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo: 0,
    pkg_c3_lo: 0,
    pkg_c3_delta: 0,
    pkg_c3_active: 0,
    pkg_c3_ema: 0,
});

fn has_rapl() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax & (1 << 4)) != 0
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
    if !has_rapl() {
        serial_println!("[msr_pkg_c3_residency] RAPL not supported, skipping init");
        return;
    }
    let (lo, _hi) = read_msr(MSR_PKG_C3_RESIDENCY);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    s.pkg_c3_lo = 0;
    s.pkg_c3_delta = 0;
    s.pkg_c3_active = 0;
    s.pkg_c3_ema = 0;
    serial_println!("[msr_pkg_c3_residency] init: last_lo={}", lo);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_PKG_C3_RESIDENCY);

    let mut s = MODULE.lock();

    // pkg_c3_lo: (lo & 0xFFFF) * 1000 / 65535, min 1000
    let raw_lo = lo & 0xFFFF;
    let pkg_c3_lo = ((raw_lo as u32).saturating_mul(1000) / 65535).min(1000) as u16;

    // pkg_c3_delta: wrapping delta of lo
    let delta = lo.wrapping_sub(s.last_lo);
    let pkg_c3_delta = if delta >= 65536 {
        1000
    } else {
        ((delta as u32).saturating_mul(1000) / 65536).min(1000) as u16
    };

    // pkg_c3_active: delta > 0 → 1000 else 0
    let pkg_c3_active: u16 = if delta > 0 { 1000 } else { 0 };

    // pkg_c3_ema: EMA of pkg_c3_delta
    let pkg_c3_ema = ema(s.pkg_c3_ema, pkg_c3_delta);

    s.last_lo = lo;
    s.pkg_c3_lo = pkg_c3_lo;
    s.pkg_c3_delta = pkg_c3_delta;
    s.pkg_c3_active = pkg_c3_active;
    s.pkg_c3_ema = pkg_c3_ema;

    serial_println!(
        "[msr_pkg_c3_residency] age={} lo={} delta={} c3_lo={} c3_delta={} active={} ema={}",
        age, lo, delta, pkg_c3_lo, pkg_c3_delta, pkg_c3_active, pkg_c3_ema
    );
}

pub fn get_pkg_c3_lo() -> u16 {
    MODULE.lock().pkg_c3_lo
}

pub fn get_pkg_c3_delta() -> u16 {
    MODULE.lock().pkg_c3_delta
}

pub fn get_pkg_c3_active() -> u16 {
    MODULE.lock().pkg_c3_active
}

pub fn get_pkg_c3_ema() -> u16 {
    MODULE.lock().pkg_c3_ema
}

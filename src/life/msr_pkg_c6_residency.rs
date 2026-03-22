#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_PKG_C6_RESIDENCY: u32 = 0x60F;
const TICK_GATE: u32 = 4000;

pub struct State {
    last_lo: u32,
    last_hi: u32,
    pkg_c6_lo: u16,
    pkg_c6_delta_lo: u16,
    pkg_c6_active: u16,
    pkg_c6_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo: 0,
    last_hi: 0,
    pkg_c6_lo: 0,
    pkg_c6_delta_lo: 0,
    pkg_c6_active: 0,
    pkg_c6_ema: 0,
});

fn has_rapl() -> bool {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 4) & 1 == 1
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
        serial_println!("[msr_pkg_c6_residency] RAPL not supported; module inactive");
        return;
    }
    let (lo, hi) = read_msr(MSR_PKG_C6_RESIDENCY);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    s.last_hi = hi;
    serial_println!("[msr_pkg_c6_residency] init: lo={} hi={}", lo, hi);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let (lo, hi) = read_msr(MSR_PKG_C6_RESIDENCY);

    let mut s = MODULE.lock();

    // pkg_c6_lo: raw low 16 bits of current counter, scaled to 0-1000
    let lo16 = lo & 0xFFFF;
    let pkg_c6_lo = ((lo16 * 1000) / 65535).min(1000) as u16;

    // delta of low 32 bits between ticks (wrapping)
    let delta_lo = lo.wrapping_sub(s.last_lo);

    // Scale delta: saturate if >= 65536, else scale to 0-1000
    let pkg_c6_delta_lo: u16 = if delta_lo >= 65536 {
        1000u16
    } else {
        ((delta_lo * 1000) / 65536) as u16
    };

    // pkg_c6_active: 1000 if any delta, 0 otherwise
    let pkg_c6_active: u16 = if delta_lo > 0 { 1000 } else { 0 };

    // EMA of delta_lo signal
    let pkg_c6_ema = ema(s.pkg_c6_ema, pkg_c6_delta_lo);

    s.last_lo = lo;
    s.last_hi = hi;
    s.pkg_c6_lo = pkg_c6_lo;
    s.pkg_c6_delta_lo = pkg_c6_delta_lo;
    s.pkg_c6_active = pkg_c6_active;
    s.pkg_c6_ema = pkg_c6_ema;

    serial_println!(
        "[msr_pkg_c6_residency] age={} lo={} delta={} active={} ema={}",
        age, pkg_c6_lo, pkg_c6_delta_lo, pkg_c6_active, pkg_c6_ema
    );
}

pub fn get_pkg_c6_lo() -> u16 {
    MODULE.lock().pkg_c6_lo
}

pub fn get_pkg_c6_delta_lo() -> u16 {
    MODULE.lock().pkg_c6_delta_lo
}

pub fn get_pkg_c6_active() -> u16 {
    MODULE.lock().pkg_c6_active
}

pub fn get_pkg_c6_ema() -> u16 {
    MODULE.lock().pkg_c6_ema
}

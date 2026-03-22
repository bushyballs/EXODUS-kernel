#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_PKG_C2_RESIDENCY: u32 = 0x60D;
const TICK_GATE: u32 = 3500;

pub struct State {
    pkg_c2_lo:     u16,
    pkg_c2_delta:  u16,
    pkg_c2_active: u16,
    pkg_c2_ema:    u16,
    last_lo:       u32,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    pkg_c2_lo:     0,
    pkg_c2_delta:  0,
    pkg_c2_active: 0,
    pkg_c2_ema:    0,
    last_lo:       0,
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
        serial_println!("[msr_pkg_c2_residency] RAPL not present — module disabled");
        return;
    }
    let (lo, _hi) = read_msr(MSR_PKG_C2_RESIDENCY);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    serial_println!("[msr_pkg_c2_residency] init: last_lo={}", lo);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_PKG_C2_RESIDENCY);
    let mut s = MODULE.lock();

    // pkg_c2_lo: raw low 16 bits scaled to 0-1000
    let lo16 = (lo & 0xFFFF) as u32;
    let new_lo_sig = ((lo16.saturating_mul(1000)) / 65535).min(1000) as u16;

    // pkg_c2_delta: delta of low 32 bits between ticks
    let delta = lo.wrapping_sub(s.last_lo);
    let new_delta_sig = if delta >= 65536 {
        1000u16
    } else {
        ((delta.saturating_mul(1000)) / 65536).min(1000) as u16
    };

    // pkg_c2_active: 1000 if any delta, else 0
    let new_active = if delta > 0 { 1000u16 } else { 0u16 };

    // pkg_c2_ema: EMA of pkg_c2_delta
    let new_ema = ema(s.pkg_c2_ema, new_delta_sig);

    s.pkg_c2_lo     = new_lo_sig;
    s.pkg_c2_delta  = new_delta_sig;
    s.pkg_c2_active = new_active;
    s.pkg_c2_ema    = new_ema;
    s.last_lo       = lo;

    serial_println!(
        "[msr_pkg_c2_residency] tick {}: lo={} delta={} active={} ema={}",
        age, new_lo_sig, new_delta_sig, new_active, new_ema
    );
}

pub fn get_pkg_c2_lo() -> u16 {
    MODULE.lock().pkg_c2_lo
}

pub fn get_pkg_c2_delta() -> u16 {
    MODULE.lock().pkg_c2_delta
}

pub fn get_pkg_c2_active() -> u16 {
    MODULE.lock().pkg_c2_active
}

pub fn get_pkg_c2_ema() -> u16 {
    MODULE.lock().pkg_c2_ema
}

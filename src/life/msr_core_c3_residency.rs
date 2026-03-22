#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_CORE_C3_RESIDENCY: u32 = 0x3FD;
const TICK_GATE: u32 = 3000;

pub struct State {
    last_lo: u32,
    core_c3_lo: u16,
    core_c3_delta: u16,
    core_c3_active: u16,
    core_c3_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo: 0,
    core_c3_lo: 0,
    core_c3_delta: 0,
    core_c3_active: 0,
    core_c3_ema: 0,
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
        serial_println!("[msr_core_c3_residency] RAPL not supported, skipping init");
        return;
    }
    let (lo, _hi) = read_msr(MSR_CORE_C3_RESIDENCY);
    let mut s = MODULE.lock();
    s.last_lo = lo;
    serial_println!("[msr_core_c3_residency] init: last_lo={}", lo);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_rapl() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_CORE_C3_RESIDENCY);
    let mut s = MODULE.lock();

    // core_c3_lo: (lo & 0xFFFF) * 1000 / 65535, min 1000
    let lo_low16 = (lo & 0xFFFF) as u32;
    let core_c3_lo = ((lo_low16 * 1000) / 65535).min(1000) as u16;

    // core_c3_delta: wrapping delta of lo. if >= 65536 → 1000, else (delta * 1000 / 65536).min(1000)
    let delta = lo.wrapping_sub(s.last_lo);
    let core_c3_delta: u16 = if delta >= 65536 {
        1000
    } else {
        ((delta * 1000) / 65536).min(1000) as u16
    };

    // core_c3_active: delta > 0 → 1000 else 0
    let core_c3_active: u16 = if delta > 0 { 1000 } else { 0 };

    // core_c3_ema: EMA of core_c3_delta
    let core_c3_ema = ema(s.core_c3_ema, core_c3_delta);

    s.last_lo = lo;
    s.core_c3_lo = core_c3_lo;
    s.core_c3_delta = core_c3_delta;
    s.core_c3_active = core_c3_active;
    s.core_c3_ema = core_c3_ema;

    serial_println!(
        "[msr_core_c3_residency] tick={} lo={} c3_lo={} delta={} active={} ema={}",
        age, lo, core_c3_lo, core_c3_delta, core_c3_active, core_c3_ema
    );
}

pub fn get_core_c3_lo() -> u16 {
    MODULE.lock().core_c3_lo
}

pub fn get_core_c3_delta() -> u16 {
    MODULE.lock().core_c3_delta
}

pub fn get_core_c3_active() -> u16 {
    MODULE.lock().core_c3_active
}

pub fn get_core_c3_ema() -> u16 {
    MODULE.lock().core_c3_ema
}

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_CORE_C6_RESIDENCY: u32 = 0x3FE;
const TICK_GATE: u32 = 3000;

pub struct State {
    core_c6_lo:    u16,
    core_c6_delta: u16,
    core_c6_active: u16,
    core_c6_ema:   u16,
    last_lo:       u32,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    core_c6_lo:    0,
    core_c6_delta: 0,
    core_c6_active: 0,
    core_c6_ema:   0,
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

fn read_msr(addr: u32) -> u64 {
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
    ((hi as u64) << 32) | (lo as u64)
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if !has_rapl() {
        serial_println!("[msr_core_c6_residency] RAPL not supported; skipping init");
        return;
    }

    let raw = read_msr(MSR_CORE_C6_RESIDENCY);
    let lo32 = (raw & 0xFFFF_FFFF) as u32;

    let mut s = MODULE.lock();
    s.last_lo = lo32;
    serial_println!("[msr_core_c6_residency] init seeded last_lo={}", lo32);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_rapl() {
        return;
    }

    let raw = read_msr(MSR_CORE_C6_RESIDENCY);
    let lo32 = (raw & 0xFFFF_FFFF) as u32;
    let lo16 = (raw & 0xFFFF) as u32;

    let mut s = MODULE.lock();

    // core_c6_lo: low 16 bits scaled to 0-1000
    let new_lo = ((lo16 * 1000) / 65535).min(1000) as u16;

    // core_c6_delta: delta of low 32 bits between ticks
    let delta = lo32.wrapping_sub(s.last_lo);
    let new_delta: u16 = if delta >= 65536 {
        1000
    } else {
        ((delta * 1000) / 65536).min(1000) as u16
    };

    // core_c6_active: any movement at all
    let new_active: u16 = if delta > 0 { 1000 } else { 0 };

    // EMA of delta
    let new_ema = ema(s.core_c6_ema, new_delta);

    s.last_lo       = lo32;
    s.core_c6_lo    = new_lo;
    s.core_c6_delta = new_delta;
    s.core_c6_active = new_active;
    s.core_c6_ema   = new_ema;

    serial_println!(
        "[msr_core_c6_residency] tick={} lo={} delta={} active={} ema={}",
        age, new_lo, new_delta, new_active, new_ema
    );
}

pub fn get_core_c6_lo() -> u16 {
    MODULE.lock().core_c6_lo
}

pub fn get_core_c6_delta() -> u16 {
    MODULE.lock().core_c6_delta
}

pub fn get_core_c6_active() -> u16 {
    MODULE.lock().core_c6_active
}

pub fn get_core_c6_ema() -> u16 {
    MODULE.lock().core_c6_ema
}

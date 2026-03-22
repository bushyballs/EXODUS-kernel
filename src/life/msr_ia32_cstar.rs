#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const IA32_CSTAR: u32 = 0xC000_0083;

pub struct State {
    cstar_lo:         u16,
    cstar_hi:         u16,
    cstar_configured: u16,
    cstar_ema:        u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    cstar_lo:         0,
    cstar_hi:         0,
    cstar_configured: 0,
    cstar_ema:        0,
});

fn has_syscall() -> bool {
    let max_ext: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x8000_0000u32 => max_ext,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_ext < 0x8000_0001 {
        return false;
    }
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0x8000_0001u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 11) & 1 == 1
}

fn read_cstar() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") IA32_CSTAR,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

fn scale(val: u32) -> u16 {
    ((val as u64).saturating_mul(1000) / 65535).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.cstar_lo         = 0;
    s.cstar_hi         = 0;
    s.cstar_configured = 0;
    s.cstar_ema        = 0;
    serial_println!("[msr_ia32_cstar] init");
}

pub fn tick(age: u32) {
    if age % 15000 != 0 {
        return;
    }

    if !has_syscall() {
        serial_println!("[msr_ia32_cstar] SYSCALL not supported — skip");
        return;
    }

    let (lo, hi) = read_cstar();

    let raw_lo  = (lo >> 16) & 0xFFFF;
    let raw_hi  = hi & 0xFFFF;

    let new_lo          = scale(raw_lo);
    let new_hi          = scale(raw_hi);
    let new_configured  = if lo != 0 || hi != 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();

    let new_ema = ema(s.cstar_ema, new_lo);

    s.cstar_lo         = new_lo;
    s.cstar_hi         = new_hi;
    s.cstar_configured = new_configured;
    s.cstar_ema        = new_ema;

    serial_println!(
        "[msr_ia32_cstar] tick={} lo={} hi={} configured={} ema={}",
        age, new_lo, new_hi, new_configured, new_ema
    );
}

pub fn get_cstar_lo() -> u16 {
    MODULE.lock().cstar_lo
}

pub fn get_cstar_hi() -> u16 {
    MODULE.lock().cstar_hi
}

pub fn get_cstar_configured() -> u16 {
    MODULE.lock().cstar_configured
}

pub fn get_cstar_ema() -> u16 {
    MODULE.lock().cstar_ema
}

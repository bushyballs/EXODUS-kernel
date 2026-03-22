#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_STAR: u32 = 0xC000_0081;

pub struct State {
    star_kernel_cs: u16,
    star_user_cs:   u16,
    star_configured: u16,
    star_ema:       u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    star_kernel_cs:  0,
    star_user_cs:    0,
    star_configured: 0,
    star_ema:        0,
});

// ── CPUID guard ────────────────────────────────────────────────────────────────

fn has_syscall() -> bool {
    let max_ext: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x80000000u32 => max_ext,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_ext < 0x80000001 {
        return false;
    }
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0x80000001u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem)
        );
    }
    (edx >> 11) & 1 == 1
}

// ── MSR read ───────────────────────────────────────────────────────────────────

fn read_star() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_STAR,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── EMA ────────────────────────────────────────────────────────────────────────

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Signal scaling ─────────────────────────────────────────────────────────────

fn scale_selector(val: u32) -> u16 {
    ((val * 1000) / 65535).min(1000) as u16
}

// ── Public interface ───────────────────────────────────────────────────────────

pub fn init() {
    if !has_syscall() {
        serial_println!("[msr_ia32_star] SYSCALL/SYSRET not supported — module inactive");
        return;
    }
    let (_, hi) = read_star();
    let kernel_cs_raw = hi & 0xFFFF;
    let user_cs_raw   = (hi >> 16) & 0xFFFF;
    let configured    = if hi != 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();
    s.star_kernel_cs  = scale_selector(kernel_cs_raw);
    s.star_user_cs    = scale_selector(user_cs_raw);
    s.star_configured = configured;
    s.star_ema        = configured;

    serial_println!(
        "[msr_ia32_star] init: kernel_cs={} user_cs={} configured={}",
        s.star_kernel_cs,
        s.star_user_cs,
        s.star_configured
    );
}

pub fn tick(age: u32) {
    if age % 15000 != 0 {
        return;
    }
    if !has_syscall() {
        return;
    }

    let (_, hi) = read_star();
    let kernel_cs_raw = hi & 0xFFFF;
    let user_cs_raw   = (hi >> 16) & 0xFFFF;
    let configured    = if hi != 0 { 1000u16 } else { 0u16 };

    let mut s = MODULE.lock();
    s.star_kernel_cs  = scale_selector(kernel_cs_raw);
    s.star_user_cs    = scale_selector(user_cs_raw);
    s.star_configured = configured;
    s.star_ema        = ema(s.star_ema, configured);

    serial_println!(
        "[msr_ia32_star] tick {}: kernel_cs={} user_cs={} configured={} ema={}",
        age,
        s.star_kernel_cs,
        s.star_user_cs,
        s.star_configured,
        s.star_ema
    );
}

// ── Getters ────────────────────────────────────────────────────────────────────

pub fn get_star_kernel_cs() -> u16 {
    MODULE.lock().star_kernel_cs
}

pub fn get_star_user_cs() -> u16 {
    MODULE.lock().star_user_cs
}

pub fn get_star_configured() -> u16 {
    MODULE.lock().star_configured
}

pub fn get_star_ema() -> u16 {
    MODULE.lock().star_ema
}

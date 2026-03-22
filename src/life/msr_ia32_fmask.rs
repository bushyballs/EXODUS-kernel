#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_FMASK: u32 = 0xC0000084;
const TICK_GATE: u32 = 15000;

pub struct State {
    fmask_bits:    u16,
    fmask_if_clear: u16,
    fmask_df_clear: u16,
    fmask_ema:     u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    fmask_bits:    0,
    fmask_if_clear: 0,
    fmask_df_clear: 0,
    fmask_ema:     0,
});

// ── CPUID guard ──────────────────────────────────────────────────────────────

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

// ── MSR read ─────────────────────────────────────────────────────────────────

fn read_msr(addr: u32) -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") addr,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0F0F_0F0F;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

// ── Public interface ──────────────────────────────────────────────────────────

pub fn init() {
    if !has_syscall() {
        serial_println!("[msr_ia32_fmask] SYSCALL not supported — module idle");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_FMASK);
    let mut s = MODULE.lock();
    s.fmask_bits    = compute_fmask_bits(lo);
    s.fmask_if_clear = compute_fmask_if_clear(lo);
    s.fmask_df_clear = compute_fmask_df_clear(lo);
    s.fmask_ema     = s.fmask_bits;
    serial_println!(
        "[msr_ia32_fmask] init: lo=0x{:08x} bits={} if_clear={} df_clear={}",
        lo, s.fmask_bits, s.fmask_if_clear, s.fmask_df_clear
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_syscall() {
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_FMASK);
    let bits     = compute_fmask_bits(lo);
    let if_clear = compute_fmask_if_clear(lo);
    let df_clear = compute_fmask_df_clear(lo);

    let mut s = MODULE.lock();
    s.fmask_ema      = ema(s.fmask_ema, bits);
    s.fmask_bits     = bits;
    s.fmask_if_clear = if_clear;
    s.fmask_df_clear = df_clear;
    serial_println!(
        "[msr_ia32_fmask] tick {}: bits={} if_clear={} df_clear={} ema={}",
        age, bits, if_clear, df_clear, s.fmask_ema
    );
}

pub fn get_fmask_bits() -> u16 {
    MODULE.lock().fmask_bits
}

pub fn get_fmask_if_clear() -> u16 {
    MODULE.lock().fmask_if_clear
}

pub fn get_fmask_df_clear() -> u16 {
    MODULE.lock().fmask_df_clear
}

pub fn get_fmask_ema() -> u16 {
    MODULE.lock().fmask_ema
}

// ── Signal computation (private) ──────────────────────────────────────────────

fn compute_fmask_bits(lo: u32) -> u16 {
    let count = popcount(lo);
    ((count * 1000 / 32) as u16).min(1000)
}

fn compute_fmask_if_clear(lo: u32) -> u16 {
    if (lo >> 9) & 1 == 1 { 1000 } else { 0 }
}

fn compute_fmask_df_clear(lo: u32) -> u16 {
    if (lo >> 10) & 1 == 1 { 1000 } else { 0 }
}

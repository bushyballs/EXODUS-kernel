#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_TURBO_RATIO_LIMIT: u32 = 0x1AD;
const TICK_GATE: u32 = 5000;
const RATIO_MAX: u32 = 80;
const SPREAD_MAX: u32 = 20;

pub struct State {
    pub turbo_1c:     u16,
    pub turbo_max:    u16,
    pub turbo_spread: u16,
    pub turbo_ema:    u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    turbo_1c:     0,
    turbo_max:    0,
    turbo_spread: 0,
    turbo_ema:    0,
});

fn has_turbo() -> bool {
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
    (eax >> 1) & 1 == 1
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

fn scale_ratio(ratio: u32) -> u16 {
    (ratio.wrapping_mul(1000) / RATIO_MAX).min(1000) as u16
}

fn scale_spread(diff: u32) -> u16 {
    (diff.wrapping_mul(1000) / SPREAD_MAX).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    let mut s = MODULE.lock();
    s.turbo_1c     = 0;
    s.turbo_max    = 0;
    s.turbo_spread = 0;
    s.turbo_ema    = 0;
    serial_println!("[msr_ia32_turbo_ratio_limit] init");
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_turbo() {
        return;
    }

    let (lo, hi) = read_msr(MSR_TURBO_RATIO_LIMIT);

    let r1c: u32 = (lo)        & 0xFF;
    let r2c: u32 = (lo >>  8)  & 0xFF;
    let r3c: u32 = (lo >> 16)  & 0xFF;
    let r4c: u32 = (lo >> 24)  & 0xFF;
    let r5c: u32 = (hi)        & 0xFF;
    let r6c: u32 = (hi >>  8)  & 0xFF;
    let r7c: u32 = (hi >> 16)  & 0xFF;
    let r8c: u32 = (hi >> 24)  & 0xFF;

    let all = [r1c, r2c, r3c, r4c, r5c, r6c, r7c, r8c];

    let mut max_ratio: u32 = 0;
    for &r in &all {
        if r > max_ratio {
            max_ratio = r;
        }
    }

    let spread: u32 = if r1c >= r8c { r1c - r8c } else { 0 };

    let turbo_1c     = scale_ratio(r1c);
    let turbo_max    = scale_ratio(max_ratio);
    let turbo_spread = scale_spread(spread);

    let mut s = MODULE.lock();
    s.turbo_1c     = turbo_1c;
    s.turbo_max    = turbo_max;
    s.turbo_spread = turbo_spread;
    s.turbo_ema    = ema(s.turbo_ema, turbo_1c);

    serial_println!(
        "[msr_ia32_turbo_ratio_limit] 1c={} max={} spread={} ema={}",
        s.turbo_1c, s.turbo_max, s.turbo_spread, s.turbo_ema
    );
}

pub fn get_turbo_1c() -> u16 {
    MODULE.lock().turbo_1c
}

pub fn get_turbo_max() -> u16 {
    MODULE.lock().turbo_max
}

pub fn get_turbo_spread() -> u16 {
    MODULE.lock().turbo_spread
}

pub fn get_turbo_ema() -> u16 {
    MODULE.lock().turbo_ema
}

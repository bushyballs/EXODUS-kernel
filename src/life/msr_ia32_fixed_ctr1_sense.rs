#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_FIXED_CTR1: u32 = 0x30A;
const TICK_GATE: u32 = 250;

pub struct State {
    last_lo: u32,
    clk_lo: u16,
    clk_delta: u16,
    clk_high_util: u16,
    clk_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo: 0,
    clk_lo: 0,
    clk_delta: 0,
    clk_high_util: 0,
    clk_ema: 0,
});

pub fn init() {
    serial_println!("[msr_ia32_fixed_ctr1_sense] init");
    let mut s = MODULE.lock();
    s.last_lo = 0;
    s.clk_lo = 0;
    s.clk_delta = 0;
    s.clk_high_util = 0;
    s.clk_ema = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_pdcm() || perf_version() < 2 {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_FIXED_CTR1);

    let mut s = MODULE.lock();

    // clk_lo: (lo & 0xFFFF) * 1000 / 65535
    let raw_lo = lo & 0xFFFF;
    let clk_lo = ((raw_lo as u32).saturating_mul(1000) / 65535) as u16;

    // clk_delta: wrapping delta of lo
    let delta = lo.wrapping_sub(s.last_lo);
    let clk_delta = if delta >= 65536 {
        1000u16
    } else {
        ((delta.saturating_mul(1000)) / 65536).min(1000) as u16
    };

    // clk_high_util: if clk_delta > 800 → 1000, else 0
    let clk_high_util: u16 = if clk_delta > 800 { 1000 } else { 0 };

    // clk_ema: EMA of clk_delta
    let clk_ema = ema(s.clk_ema, clk_delta);

    s.last_lo = lo;
    s.clk_lo = clk_lo;
    s.clk_delta = clk_delta;
    s.clk_high_util = clk_high_util;
    s.clk_ema = clk_ema;

    serial_println!(
        "[msr_ia32_fixed_ctr1_sense] clk_lo={} delta={} high_util={} ema={}",
        clk_lo, clk_delta, clk_high_util, clk_ema
    );
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
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
            options(nostack, nomem)
        );
    }
    (lo, hi)
}

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    (ecx >> 15) & 1 == 1
}

fn perf_version() -> u32 {
    let max_leaf: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0u32 => max_leaf,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    if max_leaf < 0xA {
        return 0;
    }
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 0xAu32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem)
        );
    }
    eax & 0xFF
}

pub fn get_clk_lo() -> u16 {
    MODULE.lock().clk_lo
}

pub fn get_clk_delta() -> u16 {
    MODULE.lock().clk_delta
}

pub fn get_clk_high_util() -> u16 {
    MODULE.lock().clk_high_util
}

pub fn get_clk_ema() -> u16 {
    MODULE.lock().clk_ema
}

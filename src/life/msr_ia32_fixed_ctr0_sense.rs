#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_FIXED_CTR0: u32 = 0x309;
const TICK_GATE: u32 = 200;

pub struct State {
    last_lo:     u32,
    instr_lo:    u16,
    instr_delta: u16,
    instr_burst: u16,
    instr_ema:   u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    last_lo:     0,
    instr_lo:    0,
    instr_delta: 0,
    instr_burst: 0,
    instr_ema:   0,
});

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

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if !has_pdcm() || perf_version() < 2 {
        serial_println!("[msr_ia32_fixed_ctr0_sense] PDCM or perf version check failed — module disabled");
        return;
    }
    let (lo, _hi) = read_msr(MSR_IA32_FIXED_CTR0);
    let mut s = MODULE.lock();
    s.last_lo     = lo;
    s.instr_lo    = 0;
    s.instr_delta = 0;
    s.instr_burst = 0;
    s.instr_ema   = 0;
    serial_println!("[msr_ia32_fixed_ctr0_sense] init OK — IA32_FIXED_CTR0=0x{:08x}", lo);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_pdcm() || perf_version() < 2 {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_FIXED_CTR0);

    let mut s = MODULE.lock();

    // instr_lo: low 16 bits of counter scaled to 0-1000
    let raw_lo = lo & 0xFFFF;
    let instr_lo = (raw_lo as u32 * 1000 / 65535) as u16;

    // instr_delta: wrapping difference of lo since last tick, scaled to 0-1000
    let delta = lo.wrapping_sub(s.last_lo);
    let instr_delta = if delta >= 65536 {
        1000
    } else {
        ((delta * 1000 / 65536) as u16).min(1000)
    };

    // instr_burst: high activity burst detector
    let instr_burst = if instr_delta > 750 { 1000 } else { 0 };

    // instr_ema: EMA of instr_delta
    let instr_ema = ema(s.instr_ema, instr_delta);

    s.last_lo     = lo;
    s.instr_lo    = instr_lo;
    s.instr_delta = instr_delta;
    s.instr_burst = instr_burst;
    s.instr_ema   = instr_ema;
}

pub fn get_instr_lo() -> u16 {
    MODULE.lock().instr_lo
}

pub fn get_instr_delta() -> u16 {
    MODULE.lock().instr_delta
}

pub fn get_instr_burst() -> u16 {
    MODULE.lock().instr_burst
}

pub fn get_instr_ema() -> u16 {
    MODULE.lock().instr_ema
}

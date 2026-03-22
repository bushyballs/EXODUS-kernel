#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

pub struct State {
    tsc_ratio_numer: u16,
    tsc_ratio_denom: u16,
    crystal_mhz:     u16,
    tsc_crystal_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    tsc_ratio_numer: 0,
    tsc_ratio_denom: 0,
    crystal_mhz:     0,
    tsc_crystal_ema: 0,
});

pub fn init() {
    serial_println!("[cpuid_tsc_crystal] init");
    let mut s = MODULE.lock();
    s.tsc_ratio_numer = 0;
    s.tsc_ratio_denom = 0;
    s.crystal_mhz     = 0;
    s.tsc_crystal_ema = 0;
}

pub fn tick(age: u32) {
    if age % 20000 != 0 {
        return;
    }

    if max_cpuid_leaf() < 0x15 {
        return;
    }

    let (eax_denom, ebx_numer, ecx_crystal_hz) = read_cpuid_15();

    let (numer_sig, denom_sig, mhz_sig) = if ebx_numer == 0 {
        (0u16, 0u16, 0u16)
    } else {
        let numer_sig = ((ebx_numer as u64) * 1000 / 1000).min(1000) as u16;
        let denom_sig = ((eax_denom as u64) * 1000 / 100).min(1000) as u16;
        let mhz       = ecx_crystal_hz / 1_000_000;
        let mhz_sig   = ((mhz as u64) * 1000 / 50).min(1000) as u16;
        (numer_sig, denom_sig, mhz_sig)
    };

    let mut s = MODULE.lock();

    s.tsc_ratio_numer = numer_sig;
    s.tsc_ratio_denom = denom_sig;
    s.crystal_mhz     = mhz_sig;

    let old = s.tsc_crystal_ema;
    let new = mhz_sig;
    s.tsc_crystal_ema = (((old as u32).wrapping_mul(7).saturating_add(new as u32)) / 8) as u16;

    serial_println!(
        "[cpuid_tsc_crystal] numer={} denom={} crystal_mhz={} ema={}",
        s.tsc_ratio_numer,
        s.tsc_ratio_denom,
        s.crystal_mhz,
        s.tsc_crystal_ema,
    );
}

// ---------------------------------------------------------------------------
// CPUID helpers
// ---------------------------------------------------------------------------

fn max_cpuid_leaf() -> u32 {
    let eax: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 0u32 => eax,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    eax
}

fn read_cpuid_15() -> (u32, u32, u32) {
    let eax: u32;
    let ebx: u32;
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "mov {ebx_out:e}, ebx", "pop rbx",
            inout("eax") 0x15u32 => eax,
            ebx_out = out(reg) ebx,
            out("ecx") ecx,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    (eax, ebx, ecx)
}

// ---------------------------------------------------------------------------
// Getters
// ---------------------------------------------------------------------------

pub fn get_tsc_ratio_numer() -> u16 {
    MODULE.lock().tsc_ratio_numer
}

pub fn get_tsc_ratio_denom() -> u16 {
    MODULE.lock().tsc_ratio_denom
}

pub fn get_crystal_mhz() -> u16 {
    MODULE.lock().crystal_mhz
}

pub fn get_tsc_crystal_ema() -> u16 {
    MODULE.lock().tsc_crystal_ema
}

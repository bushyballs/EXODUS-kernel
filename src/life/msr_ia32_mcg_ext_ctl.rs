#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_MCG_EXT_CTL: u32 = 0x4D0;
const MSR_IA32_MCG_CAP: u32 = 0x179;
const TICK_GATE: u32 = 12000;

pub struct State {
    lmce_en: u16,
    mcg_ext_raw: u16,
    lmce_capable: u16,
    mcg_ext_ema: u16,
    lmce_capable_latch: bool,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    lmce_en: 0,
    mcg_ext_raw: 0,
    lmce_capable: 0,
    mcg_ext_ema: 0,
    lmce_capable_latch: false,
});

fn has_mca() -> bool {
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") edx,
            options(nostack, nomem),
        );
    }
    (edx >> 14) & 1 == 1
}

fn has_lmce() -> bool {
    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_MCG_CAP,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    (lo >> 27) & 1 == 1
}

fn read_mcg_ext_ctl() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_MCG_EXT_CTL,
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
    let latch = has_lmce();
    let mut s = MODULE.lock();
    s.lmce_capable_latch = latch;
    s.lmce_capable = if latch { 1000 } else { 0 };
    serial_println!("[msr_ia32_mcg_ext_ctl] init: lmce_capable_latch={}", latch);
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }
    if !has_mca() || !has_lmce() {
        return;
    }

    let (lo, _hi) = read_mcg_ext_ctl();

    let lmce_en: u16 = if (lo & 1) != 0 { 1000 } else { 0 };

    let raw_byte = (lo & 0xFF) as u32;
    let mcg_ext_raw: u16 = ((raw_byte * 1000) / 255).min(1000) as u16;

    let mut s = MODULE.lock();
    s.lmce_en = lmce_en;
    s.mcg_ext_raw = mcg_ext_raw;
    s.mcg_ext_ema = ema(s.mcg_ext_ema, lmce_en);

    serial_println!(
        "[msr_ia32_mcg_ext_ctl] tick={} lmce_en={} mcg_ext_raw={} ema={}",
        age, s.lmce_en, s.mcg_ext_raw, s.mcg_ext_ema
    );
}

pub fn get_lmce_en() -> u16 {
    MODULE.lock().lmce_en
}

pub fn get_mcg_ext_raw() -> u16 {
    MODULE.lock().mcg_ext_raw
}

pub fn get_lmce_capable() -> u16 {
    MODULE.lock().lmce_capable
}

pub fn get_mcg_ext_ema() -> u16 {
    MODULE.lock().mcg_ext_ema
}

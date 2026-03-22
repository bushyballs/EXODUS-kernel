#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_MCG_CAP: u32 = 0x179;
const MSR_IA32_MCG_CTL: u32 = 0x17B;
const TICK_GATE: u32 = 10000;

pub struct State {
    pub mcg_ctl_banks_enabled: u16,
    pub mcg_ctl_lo_raw: u16,
    pub mcg_ctl_all_enabled: u16,
    pub mcg_ctl_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    mcg_ctl_banks_enabled: 0,
    mcg_ctl_lo_raw: 0,
    mcg_ctl_all_enabled: 0,
    mcg_ctl_ema: 0,
});

fn popcount(mut v: u32) -> u32 {
    v = v - ((v >> 1) & 0x5555_5555);
    v = (v & 0x3333_3333) + ((v >> 2) & 0x3333_3333);
    v = (v + (v >> 4)) & 0x0f0f_0f0f;
    v = v.wrapping_mul(0x0101_0101) >> 24;
    v
}

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

fn has_mcg_ctl_p() -> bool {
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
    (lo >> 8) & 1 == 1
}

fn read_mcg_ctl() -> (u32, u32) {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") MSR_IA32_MCG_CTL,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }
    (lo, hi)
}

fn read_mcg_cap_bank_count() -> u32 {
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
    // bits 7:0 of MCG_CAP = MCG_Count (number of error-reporting banks)
    lo & 0xFF
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    serial_println!("[msr_ia32_mcg_ctl] init");
    let mut s = MODULE.lock();
    s.mcg_ctl_banks_enabled = 0;
    s.mcg_ctl_lo_raw = 0;
    s.mcg_ctl_all_enabled = 0;
    s.mcg_ctl_ema = 0;
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_mca() || !has_mcg_ctl_p() {
        return;
    }

    let (lo, _hi) = read_mcg_ctl();
    let bank_count = read_mcg_cap_bank_count();

    let enabled_count = popcount(lo);

    // mcg_ctl_banks_enabled: count of enabled bank bits, scaled to 0-1000
    let banks_enabled: u16 = ((enabled_count * 1000 / 32) as u16).min(1000);

    // mcg_ctl_lo_raw: popcount of lo bits, same scaling
    let lo_raw: u16 = ((popcount(lo) * 1000 / 32) as u16).min(1000);

    // mcg_ctl_all_enabled: 1000 if all implemented banks are enabled, else 0
    let all_enabled: u16 = if bank_count == 0 {
        0
    } else {
        // build a mask for the implemented banks (up to 32)
        let clamped = bank_count.min(32);
        let mask: u32 = if clamped >= 32 {
            0xFFFF_FFFF
        } else {
            (1u32 << clamped) - 1
        };
        if (lo & mask) == mask {
            1000
        } else {
            0
        }
    };

    let mut s = MODULE.lock();
    let prev_ema = s.mcg_ctl_ema;
    s.mcg_ctl_banks_enabled = banks_enabled;
    s.mcg_ctl_lo_raw = lo_raw;
    s.mcg_ctl_all_enabled = all_enabled;
    s.mcg_ctl_ema = ema(prev_ema, banks_enabled);

    serial_println!(
        "[msr_ia32_mcg_ctl] tick={} banks_enabled={} lo_raw={} all_enabled={} ema={}",
        age,
        s.mcg_ctl_banks_enabled,
        s.mcg_ctl_lo_raw,
        s.mcg_ctl_all_enabled,
        s.mcg_ctl_ema,
    );
}

pub fn get_mcg_ctl_banks_enabled() -> u16 {
    MODULE.lock().mcg_ctl_banks_enabled
}

pub fn get_mcg_ctl_lo_raw() -> u16 {
    MODULE.lock().mcg_ctl_lo_raw
}

pub fn get_mcg_ctl_all_enabled() -> u16 {
    MODULE.lock().mcg_ctl_all_enabled
}

pub fn get_mcg_ctl_ema() -> u16 {
    MODULE.lock().mcg_ctl_ema
}

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pp1_power_limit: u16,
    pp1_limit_en: u16,
    pp1_clamp: u16,
    pp1_config_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pp1_power_limit: 0,
    pp1_limit_en: 0,
    pp1_clamp: 0,
    pp1_config_ema: 0,
});

#[inline]
fn has_rapl() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 3) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_pp1_power_limit] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_rapl() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x640u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[14:0] = power limit value (0-32767 in 1/8W units)
    let raw_limit = lo & 0x7FFF;
    let pp1_power_limit = ((raw_limit.min(500)) * 2).min(1000) as u16;

    // bit 15 = power limit enable
    let pp1_limit_en: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };

    // bit 16 = clamp enable
    let pp1_clamp: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };

    let composite = (pp1_power_limit as u32 / 2)
        .saturating_add(pp1_limit_en as u32 / 4)
        .saturating_add(pp1_clamp as u32 / 4);

    let mut s = MODULE.lock();
    let pp1_config_ema = ((s.pp1_config_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.pp1_power_limit = pp1_power_limit;
    s.pp1_limit_en = pp1_limit_en;
    s.pp1_clamp = pp1_clamp;
    s.pp1_config_ema = pp1_config_ema;

    serial_println!("[msr_ia32_pp1_power_limit] age={} limit={} en={} clamp={} ema={}",
        age, pp1_power_limit, pp1_limit_en, pp1_clamp, pp1_config_ema);
}

pub fn get_pp1_power_limit() -> u16 { MODULE.lock().pp1_power_limit }
pub fn get_pp1_limit_en() -> u16 { MODULE.lock().pp1_limit_en }
pub fn get_pp1_clamp() -> u16 { MODULE.lock().pp1_clamp }
pub fn get_pp1_config_ema() -> u16 { MODULE.lock().pp1_config_ema }

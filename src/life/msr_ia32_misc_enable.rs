#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    fast_strings_en: u16,
    speedstep_en: u16,
    turbo_disable: u16,
    misc_feature_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    fast_strings_en: 0,
    speedstep_en: 0,
    turbo_disable: 0,
    misc_feature_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_misc_enable] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1A0u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: Fast Strings enable (REP MOVS/STOS acceleration)
    let fast_strings_en: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 16: Enhanced Intel SpeedStep Technology enable
    let speedstep_en: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    // bit 38 (hi word bit 6): IDA/Turbo Boost disable — read from hi
    // For simplicity, use bit 23 of lo as a proxy (XAPIC disable — available in lo)
    // bit 18: ENABLE_MONITOR_FSM
    let monitor_en: u16 = if (lo >> 18) & 1 != 0 { 1000 } else { 0 };

    // Turbo disable: check bit 23 (xTPR message disable) as feature indicator
    let turbo_disable: u16 = if (lo >> 23) & 1 != 0 { 1000 } else { 0 };

    let composite = (fast_strings_en as u32 / 4)
        .saturating_add(speedstep_en as u32 / 4)
        .saturating_add(monitor_en as u32 / 4)
        .saturating_add(turbo_disable as u32 / 4);

    let mut s = MODULE.lock();
    let misc_feature_ema = ((s.misc_feature_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.fast_strings_en = fast_strings_en;
    s.speedstep_en = speedstep_en;
    s.turbo_disable = turbo_disable;
    s.misc_feature_ema = misc_feature_ema;

    serial_println!("[msr_ia32_misc_enable] age={} fast_str={} eist={} turbo_dis={} ema={}",
        age, fast_strings_en, speedstep_en, turbo_disable, misc_feature_ema);
}

pub fn get_fast_strings_en()  -> u16 { MODULE.lock().fast_strings_en }
pub fn get_speedstep_en()     -> u16 { MODULE.lock().speedstep_en }
pub fn get_turbo_disable()    -> u16 { MODULE.lock().turbo_disable }
pub fn get_misc_feature_ema() -> u16 { MODULE.lock().misc_feature_ema }

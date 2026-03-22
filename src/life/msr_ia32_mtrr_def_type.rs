#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    mtrr_enabled: u16,
    fixed_mtrr_en: u16,
    default_type_cacheability: u16,
    mtrr_health_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    mtrr_enabled: 0,
    fixed_mtrr_en: 0,
    default_type_cacheability: 0,
    mtrr_health_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_mtrr_def_type] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x2FFu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[2:0]: default memory type (0=UC, 1=WC, 4=WT, 5=WP, 6=WB)
    let def_type = lo & 0x7;
    // WB (6) = most cacheable = healthiest for performance
    let default_type_cacheability: u16 = match def_type {
        6 => 1000,  // WB — fully cacheable
        5 => 750,   // WP — write protected
        4 => 500,   // WT — write through
        1 => 250,   // WC — write combining
        _ => 0,     // UC — uncacheable
    };

    // bit 10: MTRRs enabled globally
    let mtrr_enabled: u16 = if (lo >> 10) & 1 != 0 { 1000 } else { 0 };

    // bit 11: fixed-range MTRRs enabled
    let fixed_mtrr_en: u16 = if (lo >> 11) & 1 != 0 { 1000 } else { 0 };

    let composite = (mtrr_enabled as u32 / 3)
        .saturating_add(fixed_mtrr_en as u32 / 3)
        .saturating_add(default_type_cacheability as u32 / 3);

    let mut s = MODULE.lock();
    let mtrr_health_ema = ((s.mtrr_health_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.mtrr_enabled = mtrr_enabled;
    s.fixed_mtrr_en = fixed_mtrr_en;
    s.default_type_cacheability = default_type_cacheability;
    s.mtrr_health_ema = mtrr_health_ema;

    serial_println!("[msr_ia32_mtrr_def_type] age={} en={} fixed={} cache={} ema={}",
        age, mtrr_enabled, fixed_mtrr_en, default_type_cacheability, mtrr_health_ema);
}

pub fn get_mtrr_enabled()              -> u16 { MODULE.lock().mtrr_enabled }
pub fn get_fixed_mtrr_en()             -> u16 { MODULE.lock().fixed_mtrr_en }
pub fn get_default_type_cacheability() -> u16 { MODULE.lock().default_type_cacheability }
pub fn get_mtrr_health_ema()           -> u16 { MODULE.lock().mtrr_health_ema }

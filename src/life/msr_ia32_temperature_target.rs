#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_TEMPERATURE_TARGET: u32 = 0x1B2;
const TICK_GATE: u32 = 8000;
const TJ_MAX_SCALE: u32 = 120;
const TCC_OFFSET_SCALE: u32 = 64;

pub struct State {
    tj_max:           u16,
    tcc_offset:       u16,
    effective_target: u16,
    temp_target_ema:  u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    tj_max:           0,
    tcc_offset:       0,
    effective_target: 0,
    temp_target_ema:  0,
});

fn has_dts() -> bool {
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
    eax & 1 == 1
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

fn scale_tj_max(celsius: u32) -> u16 {
    (celsius.saturating_mul(1000) / TJ_MAX_SCALE).min(1000) as u16
}

fn scale_tcc_offset(offset: u32) -> u16 {
    (offset.saturating_mul(1000) / TCC_OFFSET_SCALE).min(1000) as u16
}

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

pub fn init() {
    if !has_dts() {
        serial_println!("[msr_ia32_temperature_target] DTS not present, module disabled");
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_TEMPERATURE_TARGET);

    let tj_max_raw      = (lo >> 24) & 0xFF;
    let tcc_offset_raw  = (lo >> 16) & 0xFF;
    let effective_raw   = tj_max_raw.saturating_sub(tcc_offset_raw);

    let tj_max_sig      = scale_tj_max(tj_max_raw);
    let tcc_offset_sig  = scale_tcc_offset(tcc_offset_raw);
    let effective_sig   = scale_tj_max(effective_raw);

    let mut s = MODULE.lock();
    s.tj_max           = tj_max_sig;
    s.tcc_offset       = tcc_offset_sig;
    s.effective_target = effective_sig;
    s.temp_target_ema  = effective_sig;

    serial_println!(
        "[msr_ia32_temperature_target] init: TJ_MAX={}C  TCC_OFFSET={}  effective={}C",
        tj_max_raw,
        tcc_offset_raw,
        effective_raw,
    );
}

pub fn tick(age: u32) {
    if age % TICK_GATE != 0 {
        return;
    }

    if !has_dts() {
        return;
    }

    let (lo, _hi) = read_msr(MSR_IA32_TEMPERATURE_TARGET);

    let tj_max_raw      = (lo >> 24) & 0xFF;
    let tcc_offset_raw  = (lo >> 16) & 0xFF;
    let effective_raw   = tj_max_raw.saturating_sub(tcc_offset_raw);

    let tj_max_sig      = scale_tj_max(tj_max_raw);
    let tcc_offset_sig  = scale_tcc_offset(tcc_offset_raw);
    let effective_sig   = scale_tj_max(effective_raw);

    let mut s = MODULE.lock();
    s.tj_max           = tj_max_sig;
    s.tcc_offset       = tcc_offset_sig;
    s.effective_target = effective_sig;
    s.temp_target_ema  = ema(s.temp_target_ema, effective_sig);

    serial_println!(
        "[msr_ia32_temperature_target] tick {}: TJ_MAX={} TCC_OFFSET={} effective={} ema={}",
        age,
        tj_max_sig,
        tcc_offset_sig,
        effective_sig,
        s.temp_target_ema,
    );
}

pub fn get_tj_max() -> u16 {
    MODULE.lock().tj_max
}

pub fn get_tcc_offset() -> u16 {
    MODULE.lock().tcc_offset
}

pub fn get_effective_target() -> u16 {
    MODULE.lock().effective_target
}

pub fn get_temp_target_ema() -> u16 {
    MODULE.lock().temp_target_ema
}

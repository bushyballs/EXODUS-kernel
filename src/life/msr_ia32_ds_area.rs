#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

const MSR_IA32_DS_AREA: u32 = 0x600;

pub struct State {
    ds_configured: u16,
    ds_lo_sense: u16,
    ds_hi_sense: u16,
    ds_area_ema: u16,
}

pub static MODULE: Mutex<State> = Mutex::new(State {
    ds_configured: 0,
    ds_lo_sense: 0,
    ds_hi_sense: 0,
    ds_area_ema: 0,
});

fn has_ds() -> bool {
    let ecx: u32;
    let edx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") edx,
            options(nostack, nomem),
        );
    }
    ((ecx >> 15) & 1 == 1) && ((edx >> 21) & 1 == 1)
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

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn scale_to_1000(val: u32) -> u16 {
    (val.saturating_mul(1000) / 65535).min(1000) as u16
}

pub fn init() {
    if !has_ds() {
        serial_println!("[msr_ia32_ds_area] DS/PEBS not supported on this CPU, module idle");
        return;
    }
    serial_println!("[msr_ia32_ds_area] DS area module initialized (PDCM+DS supported)");
}

pub fn tick(age: u32) {
    if age % 5000 != 0 {
        return;
    }
    if !has_ds() {
        return;
    }

    let (lo, hi) = read_msr(MSR_IA32_DS_AREA);

    let ds_configured: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };

    let lo_upper: u32 = (lo >> 16) & 0xFFFF;
    let ds_lo_sense: u16 = scale_to_1000(lo_upper);

    let hi_lower: u32 = hi & 0xFFFF;
    let ds_hi_sense: u16 = scale_to_1000(hi_lower);

    let mut state = MODULE.lock();
    let old_ema = state.ds_area_ema;
    let ds_area_ema = ema(old_ema, ds_configured);

    state.ds_configured = ds_configured;
    state.ds_lo_sense = ds_lo_sense;
    state.ds_hi_sense = ds_hi_sense;
    state.ds_area_ema = ds_area_ema;

    serial_println!(
        "[msr_ia32_ds_area] tick={} configured={} lo_sense={} hi_sense={} ema={}",
        age, ds_configured, ds_lo_sense, ds_hi_sense, ds_area_ema
    );
}

pub fn get_ds_configured() -> u16 {
    MODULE.lock().ds_configured
}

pub fn get_ds_lo_sense() -> u16 {
    MODULE.lock().ds_lo_sense
}

pub fn get_ds_hi_sense() -> u16 {
    MODULE.lock().ds_hi_sense
}

pub fn get_ds_area_ema() -> u16 {
    MODULE.lock().ds_area_ema
}

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    core_therm_throttle: u16,
    core_prochot: u16,
    core_temp_margin: u16,
    core_therm_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    core_therm_throttle: 0,
    core_prochot: 0,
    core_temp_margin: 0,
    core_therm_ema: 0,
});

#[inline]
fn has_dts() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_therm_status] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    if !has_dts() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x19Cu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: core thermal status (1 = throttle active)
    let core_therm_throttle: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 2: PROCHOT assertion
    let core_prochot: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    // bits[22:16]: digital readout — degrees below Tjmax
    let readout = (lo >> 16) & 0x7F;
    let core_temp_margin = ((readout * 1000) / 127).min(1000) as u16;

    // Thermal pressure composite
    let pressure = (core_therm_throttle as u32 / 3)
        .saturating_add(core_prochot as u32 / 3)
        .saturating_add((1000u32).saturating_sub(core_temp_margin as u32) / 3);

    let mut s = MODULE.lock();
    let core_therm_ema = ((s.core_therm_ema as u32).wrapping_mul(7)
        .saturating_add(pressure) / 8).min(1000) as u16;

    s.core_therm_throttle = core_therm_throttle;
    s.core_prochot = core_prochot;
    s.core_temp_margin = core_temp_margin;
    s.core_therm_ema = core_therm_ema;

    serial_println!("[msr_ia32_therm_status] age={} throttle={} prochot={} margin={} ema={}",
        age, core_therm_throttle, core_prochot, core_temp_margin, core_therm_ema);
}

pub fn get_core_therm_throttle() -> u16 { MODULE.lock().core_therm_throttle }
pub fn get_core_prochot()        -> u16 { MODULE.lock().core_prochot }
pub fn get_core_temp_margin()    -> u16 { MODULE.lock().core_temp_margin }
pub fn get_core_therm_ema()      -> u16 { MODULE.lock().core_therm_ema }

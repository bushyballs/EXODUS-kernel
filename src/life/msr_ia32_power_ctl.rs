#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    c1e_enable: u16,
    ee_enable: u16,
    dts_enable: u16,
    power_ctl_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    c1e_enable: 0,
    ee_enable: 0,
    dts_enable: 0,
    power_ctl_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_power_ctl] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1FCu32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let c1e_enable: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let ee_enable: u16 = if (lo >> 19) & 1 != 0 { 1000 } else { 0 };
    let dts_enable: u16 = if (lo >> 18) & 1 != 0 { 1000 } else { 0 };

    let composite = (c1e_enable as u32 / 3)
        .saturating_add(ee_enable as u32 / 3)
        .saturating_add(dts_enable as u32 / 3);

    let mut s = MODULE.lock();
    let power_ctl_ema = ((s.power_ctl_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.c1e_enable = c1e_enable;
    s.ee_enable = ee_enable;
    s.dts_enable = dts_enable;
    s.power_ctl_ema = power_ctl_ema;

    serial_println!("[msr_ia32_power_ctl] age={} c1e={} ee={} dts={} ema={}",
        age, c1e_enable, ee_enable, dts_enable, power_ctl_ema);
}

pub fn get_c1e_enable()    -> u16 { MODULE.lock().c1e_enable }
pub fn get_ee_enable()     -> u16 { MODULE.lock().ee_enable }
pub fn get_dts_enable()    -> u16 { MODULE.lock().dts_enable }
pub fn get_power_ctl_ema() -> u16 { MODULE.lock().power_ctl_ema }

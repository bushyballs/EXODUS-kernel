#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hdc_enable: u16,
    hdc_active: u16,
    hdc_force_idle: u16,
    hdc_pressure_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hdc_enable: 0,
    hdc_active: 0,
    hdc_force_idle: 0,
    hdc_pressure_ema: 0,
});

#[inline]
fn has_hdc() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 13) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_hdc_prochot_ctl] init"); }

pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_hdc() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x652u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0 = HDC enable
    let hdc_enable: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1 = HDC active (hardware duty cycling in progress)
    let hdc_active: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 4 = force idle
    let hdc_force_idle: u16 = if (lo >> 4) & 1 != 0 { 1000 } else { 0 };

    let composite = (hdc_enable as u32 / 4)
        .saturating_add(hdc_active as u32 / 2)
        .saturating_add(hdc_force_idle as u32 / 4);

    let mut s = MODULE.lock();
    let hdc_pressure_ema = ((s.hdc_pressure_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.hdc_enable = hdc_enable;
    s.hdc_active = hdc_active;
    s.hdc_force_idle = hdc_force_idle;
    s.hdc_pressure_ema = hdc_pressure_ema;

    serial_println!("[msr_ia32_hdc_prochot_ctl] age={} en={} active={} idle={} ema={}",
        age, hdc_enable, hdc_active, hdc_force_idle, hdc_pressure_ema);
}

pub fn get_hdc_enable() -> u16 { MODULE.lock().hdc_enable }
pub fn get_hdc_active() -> u16 { MODULE.lock().hdc_active }
pub fn get_hdc_force_idle() -> u16 { MODULE.lock().hdc_force_idle }
pub fn get_hdc_pressure_ema() -> u16 { MODULE.lock().hdc_pressure_ema }

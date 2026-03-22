#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pkg_therm_throttle: u16,
    pkg_prochot: u16,
    pkg_temp_margin: u16,
    pkg_therm_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pkg_therm_throttle: 0,
    pkg_prochot: 0,
    pkg_temp_margin: 0,
    pkg_therm_ema: 0,
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

pub fn init() { serial_println!("[msr_ia32_package_therm_status] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    if !has_dts() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1B1u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: package thermal status (1 = throttle active)
    let pkg_therm_throttle: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: PROCHOT assertion
    let pkg_prochot: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bits[22:16]: digital readout — degrees below Tjmax (0=at Tjmax/critical, 127=cold)
    let readout = (lo >> 16) & 0x7F;
    let pkg_temp_margin = ((readout * 1000) / 127).min(1000) as u16;

    // Composite thermal pressure: high = dangerous
    let pressure = (pkg_therm_throttle as u32 / 3)
        .saturating_add(pkg_prochot as u32 / 3)
        .saturating_add((1000u32).saturating_sub(pkg_temp_margin as u32) / 3);

    let mut s = MODULE.lock();
    let pkg_therm_ema = ((s.pkg_therm_ema as u32).wrapping_mul(7)
        .saturating_add(pressure) / 8).min(1000) as u16;

    s.pkg_therm_throttle = pkg_therm_throttle;
    s.pkg_prochot = pkg_prochot;
    s.pkg_temp_margin = pkg_temp_margin;
    s.pkg_therm_ema = pkg_therm_ema;

    serial_println!("[msr_ia32_package_therm_status] age={} throttle={} prochot={} margin={} ema={}",
        age, pkg_therm_throttle, pkg_prochot, pkg_temp_margin, pkg_therm_ema);
}

pub fn get_pkg_therm_throttle() -> u16 { MODULE.lock().pkg_therm_throttle }
pub fn get_pkg_prochot()        -> u16 { MODULE.lock().pkg_prochot }
pub fn get_pkg_temp_margin()    -> u16 { MODULE.lock().pkg_temp_margin }
pub fn get_pkg_therm_ema()      -> u16 { MODULE.lock().pkg_therm_ema }

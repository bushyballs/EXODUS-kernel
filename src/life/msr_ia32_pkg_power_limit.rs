#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pkg_pl1_active: u16,
    pkg_pl2_active: u16,
    pkg_pwr_clamping: u16,
    msr_ia32_pkg_power_limit_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pkg_pl1_active: 0,
    pkg_pl2_active: 0,
    pkg_pwr_clamping: 0,
    msr_ia32_pkg_power_limit_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_pkg_power_limit] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x610u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    let pkg_pl1_active: u16 = if (lo >> 15) & 1 != 0 { 1000 } else { 0 };
    let pkg_pl2_active: u16 = if (hi >> 15) & 1 != 0 { 1000 } else { 0 };
    let pkg_pwr_clamping: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };

    let composite = (pkg_pl1_active as u32 / 3)
        .saturating_add((pkg_pl2_active as u32 / 3));
        .saturating_add((pkg_pwr_clamping as u32 / 3));

    let mut s = MODULE.lock();
    let msr_ia32_pkg_power_limit_ema = ((s.msr_ia32_pkg_power_limit_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.pkg_pl1_active = pkg_pl1_active;
    s.pkg_pl2_active = pkg_pl2_active;
    s.pkg_pwr_clamping = pkg_pwr_clamping;
    s.msr_ia32_pkg_power_limit_ema = msr_ia32_pkg_power_limit_ema;

    serial_println!("[msr_ia32_pkg_power_limit] age={} active={} active={} clamping={} ema={}",
        age, pkg_pl1_active, pkg_pl2_active, pkg_pwr_clamping, msr_ia32_pkg_power_limit_ema);
}

pub fn get_pkg_pl1_active() -> u16 { MODULE.lock().pkg_pl1_active }
pub fn get_pkg_pl2_active() -> u16 { MODULE.lock().pkg_pl2_active }
pub fn get_pkg_pwr_clamping() -> u16 { MODULE.lock().pkg_pwr_clamping }
pub fn get_msr_ia32_pkg_power_limit_ema() -> u16 { MODULE.lock().msr_ia32_pkg_power_limit_ema }

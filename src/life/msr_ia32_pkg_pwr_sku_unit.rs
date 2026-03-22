#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    power_unit: u16,
    energy_unit: u16,
    time_unit: u16,
    unit_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    power_unit: 0,
    energy_unit: 0,
    time_unit: 0,
    unit_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_pkg_pwr_sku_unit] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x606u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[3:0]: Power Units (2^-N W)
    let raw_pow = lo & 0xF;
    let power_unit = ((raw_pow * 1000) / 15).min(1000) as u16;
    // bits[12:8]: Energy Status Units (2^-N J)
    let raw_eng = (lo >> 8) & 0x1F;
    let energy_unit = ((raw_eng * 1000) / 31).min(1000) as u16;
    // bits[19:16]: Time Units (2^-N s)
    let raw_time = (lo >> 16) & 0xF;
    let time_unit = ((raw_time * 1000) / 15).min(1000) as u16;

    let composite = (power_unit as u32 / 3)
        .saturating_add(energy_unit as u32 / 3)
        .saturating_add(time_unit as u32 / 3);

    let mut s = MODULE.lock();
    let unit_ema = ((s.unit_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.power_unit = power_unit;
    s.energy_unit = energy_unit;
    s.time_unit = time_unit;
    s.unit_ema = unit_ema;

    serial_println!("[msr_ia32_pkg_pwr_sku_unit] age={} pow={} eng={} time={} ema={}",
        age, power_unit, energy_unit, time_unit, unit_ema);
}

pub fn get_power_unit()  -> u16 { MODULE.lock().power_unit }
pub fn get_energy_unit() -> u16 { MODULE.lock().energy_unit }
pub fn get_time_unit()   -> u16 { MODULE.lock().time_unit }
pub fn get_unit_ema()    -> u16 { MODULE.lock().unit_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    energy_raw_lo: u16,
    energy_raw_hi: u16,
    energy_delta: u16,
    energy_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    energy_raw_lo: 0,
    energy_raw_hi: 0,
    energy_delta: 0,
    energy_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_pkg_energy_status] init"); }

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x611u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[31:0]: Total energy consumed (wrapping counter)
    let energy_raw_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let energy_raw_hi = (((lo >> 16) & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev_lo = s.energy_raw_lo;
    let energy_delta = if energy_raw_lo >= prev_lo {
        (energy_raw_lo - prev_lo).min(1000)
    } else {
        (1000u16).saturating_sub(prev_lo).saturating_add(energy_raw_lo).min(1000)
    };

    let energy_ema = ((s.energy_ema as u32).wrapping_mul(7)
        .saturating_add(energy_delta as u32) / 8).min(1000) as u16;

    s.energy_raw_lo = energy_raw_lo;
    s.energy_raw_hi = energy_raw_hi;
    s.energy_delta = energy_delta;
    s.energy_ema = energy_ema;

    serial_println!("[msr_ia32_pkg_energy_status] age={} lo={} hi={} delta={} ema={}",
        age, energy_raw_lo, energy_raw_hi, energy_delta, energy_ema);
}

pub fn get_energy_raw_lo() -> u16 { MODULE.lock().energy_raw_lo }
pub fn get_energy_raw_hi() -> u16 { MODULE.lock().energy_raw_hi }
pub fn get_energy_delta()  -> u16 { MODULE.lock().energy_delta }
pub fn get_energy_ema()    -> u16 { MODULE.lock().energy_ema }

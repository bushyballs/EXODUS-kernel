#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    msr_ia32_pp0_energy_status_lo: u16,
    msr_ia32_pp0_energy_status_hi: u16,
    msr_ia32_pp0_energy_status_delta: u16,
    msr_ia32_pp0_energy_status_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    msr_ia32_pp0_energy_status_lo: 0,
    msr_ia32_pp0_energy_status_hi: 0,
    msr_ia32_pp0_energy_status_delta: 0,
    msr_ia32_pp0_energy_status_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_pp0_energy_status] init"); }

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x639u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let msr_ia32_pp0_energy_status_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let msr_ia32_pp0_energy_status_hi = (((lo >> 16) & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.msr_ia32_pp0_energy_status_lo;
    let msr_ia32_pp0_energy_status_delta = if msr_ia32_pp0_energy_status_lo >= prev {
        (msr_ia32_pp0_energy_status_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(msr_ia32_pp0_energy_status_lo).min(1000)
    };

    let msr_ia32_pp0_energy_status_ema = ((s.msr_ia32_pp0_energy_status_ema as u32).wrapping_mul(7)
        .saturating_add(msr_ia32_pp0_energy_status_delta as u32) / 8).min(1000) as u16;

    s.msr_ia32_pp0_energy_status_lo = msr_ia32_pp0_energy_status_lo;
    s.msr_ia32_pp0_energy_status_hi = msr_ia32_pp0_energy_status_hi;
    s.msr_ia32_pp0_energy_status_delta = msr_ia32_pp0_energy_status_delta;
    s.msr_ia32_pp0_energy_status_ema = msr_ia32_pp0_energy_status_ema;

    serial_println!("[msr_ia32_pp0_energy_status] age={} lo={} hi={} delta={} ema={}",
        age, msr_ia32_pp0_energy_status_lo, msr_ia32_pp0_energy_status_hi, msr_ia32_pp0_energy_status_delta, msr_ia32_pp0_energy_status_ema);
}

pub fn get_msr_ia32_pp0_energy_status_lo()    -> u16 { MODULE.lock().msr_ia32_pp0_energy_status_lo }
pub fn get_msr_ia32_pp0_energy_status_hi()    -> u16 { MODULE.lock().msr_ia32_pp0_energy_status_hi }
pub fn get_msr_ia32_pp0_energy_status_delta() -> u16 { MODULE.lock().msr_ia32_pp0_energy_status_delta }
pub fn get_msr_ia32_pp0_energy_status_ema()   -> u16 { MODULE.lock().msr_ia32_pp0_energy_status_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    platform_energy_lo: u16,
    platform_energy_hi: u16,
    platform_energy_delta: u16,
    platform_energy_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    platform_energy_lo: 0,
    platform_energy_hi: 0,
    platform_energy_delta: 0,
    platform_energy_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_platform_energy_counter] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x64Du32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // bits[31:0]: platform energy counter (wrapping)
    let platform_energy_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let platform_energy_hi = (((lo >> 16) & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.platform_energy_lo;
    let platform_energy_delta = if platform_energy_lo >= prev {
        (platform_energy_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(platform_energy_lo).min(1000)
    };

    let platform_energy_ema = ((s.platform_energy_ema as u32).wrapping_mul(7)
        .saturating_add(platform_energy_delta as u32) / 8).min(1000) as u16;

    s.platform_energy_lo = platform_energy_lo;
    s.platform_energy_hi = platform_energy_hi;
    s.platform_energy_delta = platform_energy_delta;
    s.platform_energy_ema = platform_energy_ema;

    serial_println!("[msr_ia32_platform_energy_counter] age={} lo={} hi={} delta={} ema={}",
        age, platform_energy_lo, platform_energy_hi, platform_energy_delta, platform_energy_ema);
}

pub fn get_platform_energy_lo()    -> u16 { MODULE.lock().platform_energy_lo }
pub fn get_platform_energy_hi()    -> u16 { MODULE.lock().platform_energy_hi }
pub fn get_platform_energy_delta() -> u16 { MODULE.lock().platform_energy_delta }
pub fn get_platform_energy_ema()   -> u16 { MODULE.lock().platform_energy_ema }

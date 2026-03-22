#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    aperf_lo: u16,
    aperf_hi: u16,
    aperf_rate: u16,
    msr_ia32_aperf_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    aperf_lo: 0,
    aperf_hi: 0,
    aperf_rate: 0,
    msr_ia32_aperf_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_aperf] init"); }

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0xE8u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // Actual performance frequency clock counter
    let aperf_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let aperf_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.aperf_lo;
    let aperf_rate = if aperf_lo >= prev {
        (aperf_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(aperf_lo).min(1000)
    };

    let msr_ia32_aperf_ema = ((s.msr_ia32_aperf_ema as u32).wrapping_mul(7)
        .saturating_add(aperf_rate as u32) / 8).min(1000) as u16;

    s.aperf_lo = aperf_lo;
    s.aperf_hi = aperf_hi;
    s.aperf_rate = aperf_rate;
    s.msr_ia32_aperf_ema = msr_ia32_aperf_ema;

    serial_println!("[msr_ia32_aperf] age={} lo={} hi={} rate={} ema={}",
        age, aperf_lo, aperf_hi, aperf_rate, msr_ia32_aperf_ema);
}

pub fn get_aperf_lo()  -> u16 { MODULE.lock().aperf_lo }
pub fn get_aperf_hi()  -> u16 { MODULE.lock().aperf_hi }
pub fn get_aperf_rate() -> u16 { MODULE.lock().aperf_rate }
pub fn get_msr_ia32_aperf_ema()  -> u16 { MODULE.lock().msr_ia32_aperf_ema }

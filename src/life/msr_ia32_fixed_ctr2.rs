#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    msr_ia32_fixed_ctr2_lo: u16,
    msr_ia32_fixed_ctr2_hi: u16,
    msr_ia32_fixed_ctr2_rate: u16,
    msr_ia32_fixed_ctr2_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    msr_ia32_fixed_ctr2_lo: 0,
    msr_ia32_fixed_ctr2_hi: 0,
    msr_ia32_fixed_ctr2_rate: 0,
    msr_ia32_fixed_ctr2_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_fixed_ctr2] init"); }

pub fn tick(age: u32) {
    if age % 200 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x30Bu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // Reference cycles unhalted (ref TSC)
    let msr_ia32_fixed_ctr2_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let msr_ia32_fixed_ctr2_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.msr_ia32_fixed_ctr2_lo;
    let msr_ia32_fixed_ctr2_rate = if msr_ia32_fixed_ctr2_lo >= prev {
        (msr_ia32_fixed_ctr2_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(msr_ia32_fixed_ctr2_lo).min(1000)
    };

    let msr_ia32_fixed_ctr2_ema = ((s.msr_ia32_fixed_ctr2_ema as u32).wrapping_mul(7)
        .saturating_add(msr_ia32_fixed_ctr2_rate as u32) / 8).min(1000) as u16;

    s.msr_ia32_fixed_ctr2_lo = msr_ia32_fixed_ctr2_lo;
    s.msr_ia32_fixed_ctr2_hi = msr_ia32_fixed_ctr2_hi;
    s.msr_ia32_fixed_ctr2_rate = msr_ia32_fixed_ctr2_rate;
    s.msr_ia32_fixed_ctr2_ema = msr_ia32_fixed_ctr2_ema;

    serial_println!("[msr_ia32_fixed_ctr2] age={} lo={} hi={} rate={} ema={}",
        age, msr_ia32_fixed_ctr2_lo, msr_ia32_fixed_ctr2_hi, msr_ia32_fixed_ctr2_rate, msr_ia32_fixed_ctr2_ema);
}

pub fn get_msr_ia32_fixed_ctr2_lo()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr2_lo }
pub fn get_msr_ia32_fixed_ctr2_hi()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr2_hi }
pub fn get_msr_ia32_fixed_ctr2_rate() -> u16 { MODULE.lock().msr_ia32_fixed_ctr2_rate }
pub fn get_msr_ia32_fixed_ctr2_ema()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr2_ema }

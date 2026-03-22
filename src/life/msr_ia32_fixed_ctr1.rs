#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    msr_ia32_fixed_ctr1_lo: u16,
    msr_ia32_fixed_ctr1_hi: u16,
    msr_ia32_fixed_ctr1_rate: u16,
    msr_ia32_fixed_ctr1_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    msr_ia32_fixed_ctr1_lo: 0,
    msr_ia32_fixed_ctr1_hi: 0,
    msr_ia32_fixed_ctr1_rate: 0,
    msr_ia32_fixed_ctr1_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_fixed_ctr1] init"); }

pub fn tick(age: u32) {
    if age % 200 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x30Au32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // CPU cycles unhalted (core P-state)
    let msr_ia32_fixed_ctr1_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let msr_ia32_fixed_ctr1_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.msr_ia32_fixed_ctr1_lo;
    let msr_ia32_fixed_ctr1_rate = if msr_ia32_fixed_ctr1_lo >= prev {
        (msr_ia32_fixed_ctr1_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(msr_ia32_fixed_ctr1_lo).min(1000)
    };

    let msr_ia32_fixed_ctr1_ema = ((s.msr_ia32_fixed_ctr1_ema as u32).wrapping_mul(7)
        .saturating_add(msr_ia32_fixed_ctr1_rate as u32) / 8).min(1000) as u16;

    s.msr_ia32_fixed_ctr1_lo = msr_ia32_fixed_ctr1_lo;
    s.msr_ia32_fixed_ctr1_hi = msr_ia32_fixed_ctr1_hi;
    s.msr_ia32_fixed_ctr1_rate = msr_ia32_fixed_ctr1_rate;
    s.msr_ia32_fixed_ctr1_ema = msr_ia32_fixed_ctr1_ema;

    serial_println!("[msr_ia32_fixed_ctr1] age={} lo={} hi={} rate={} ema={}",
        age, msr_ia32_fixed_ctr1_lo, msr_ia32_fixed_ctr1_hi, msr_ia32_fixed_ctr1_rate, msr_ia32_fixed_ctr1_ema);
}

pub fn get_msr_ia32_fixed_ctr1_lo()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr1_lo }
pub fn get_msr_ia32_fixed_ctr1_hi()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr1_hi }
pub fn get_msr_ia32_fixed_ctr1_rate() -> u16 { MODULE.lock().msr_ia32_fixed_ctr1_rate }
pub fn get_msr_ia32_fixed_ctr1_ema()  -> u16 { MODULE.lock().msr_ia32_fixed_ctr1_ema }

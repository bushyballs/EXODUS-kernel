#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    mperf_lo: u16,
    mperf_hi: u16,
    mperf_rate: u16,
    msr_ia32_mperf_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    mperf_lo: 0,
    mperf_hi: 0,
    mperf_rate: 0,
    msr_ia32_mperf_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_mperf] init"); }

pub fn tick(age: u32) {
    if age % 500 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0xE7u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // Max performance frequency clock counter
    let mperf_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let mperf_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let mut s = MODULE.lock();
    let prev = s.mperf_lo;
    let mperf_rate = if mperf_lo >= prev {
        (mperf_lo - prev).min(1000)
    } else {
        (1000u16).saturating_sub(prev).saturating_add(mperf_lo).min(1000)
    };

    let msr_ia32_mperf_ema = ((s.msr_ia32_mperf_ema as u32).wrapping_mul(7)
        .saturating_add(mperf_rate as u32) / 8).min(1000) as u16;

    s.mperf_lo = mperf_lo;
    s.mperf_hi = mperf_hi;
    s.mperf_rate = mperf_rate;
    s.msr_ia32_mperf_ema = msr_ia32_mperf_ema;

    serial_println!("[msr_ia32_mperf] age={} lo={} hi={} rate={} ema={}",
        age, mperf_lo, mperf_hi, mperf_rate, msr_ia32_mperf_ema);
}

pub fn get_mperf_lo()  -> u16 { MODULE.lock().mperf_lo }
pub fn get_mperf_hi()  -> u16 { MODULE.lock().mperf_hi }
pub fn get_mperf_rate() -> u16 { MODULE.lock().mperf_rate }
pub fn get_msr_ia32_mperf_ema()  -> u16 { MODULE.lock().msr_ia32_mperf_ema }

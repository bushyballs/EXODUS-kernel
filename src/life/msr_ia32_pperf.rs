#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    pperf_delta: u16,
    pperf_rate: u16,
    pperf_trend: u16,
    pperf_ema: u16,
    last_lo: u32,
    last_hi: u32,
}

static MODULE: Mutex<State> = Mutex::new(State {
    pperf_delta: 0,
    pperf_rate: 0,
    pperf_trend: 0,
    pperf_ema: 0,
    last_lo: 0,
    last_hi: 0,
});

pub fn init() { serial_println!("[msr_ia32_pperf] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x64Eu32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    let mut s = MODULE.lock();
    let prev_lo = s.last_lo;

    // 32-bit wrapping delta — productive cycles retired since last sample
    let delta_lo = lo.wrapping_sub(prev_lo);

    // Scale: cap at 4M counts per tick → 1000 signal
    let pperf_delta = (delta_lo / 4096).min(1000) as u16;

    // Rate mirrors delta (single-sample throughput)
    let pperf_rate = pperf_delta;

    // Trend: how far above the smoothed baseline we are
    let pperf_trend: u16 = if pperf_delta >= s.pperf_ema {
        ((pperf_delta as u32).saturating_sub(s.pperf_ema as u32)).min(1000) as u16
    } else {
        0
    };

    let pperf_ema = ((s.pperf_ema as u32).wrapping_mul(7)
        .saturating_add(pperf_delta as u32) / 8) as u16;

    s.last_lo = lo;
    s.last_hi = hi;
    s.pperf_delta = pperf_delta;
    s.pperf_rate = pperf_rate;
    s.pperf_trend = pperf_trend;
    s.pperf_ema = pperf_ema;

    serial_println!("[msr_ia32_pperf] age={} delta={} rate={} trend={} ema={}",
        age, pperf_delta, pperf_rate, pperf_trend, pperf_ema);
}

pub fn get_pperf_delta() -> u16 { MODULE.lock().pperf_delta }
pub fn get_pperf_rate()  -> u16 { MODULE.lock().pperf_rate }
pub fn get_pperf_trend() -> u16 { MODULE.lock().pperf_trend }
pub fn get_pperf_ema()   -> u16 { MODULE.lock().pperf_ema }

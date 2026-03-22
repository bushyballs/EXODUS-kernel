#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    epb_hint: u16,
    epb_perf_bias: u16,
    epb_efficiency_bias: u16,
    epb_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    epb_hint: 0,
    epb_perf_bias: 0,
    epb_efficiency_bias: 0,
    epb_ema: 0,
});

#[inline]
fn has_epb() -> bool {
    let ecx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => _,
            lateout("ecx") ecx, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (ecx >> 3) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_energy_perf_bias] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_epb() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1B0u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bits[3:0]: EPB hint — 0=max performance, 15=max power savings
    let raw = lo & 0xF;

    // Raw 0-15 → scaled 0-1000
    let epb_hint = ((raw * 1000) / 15) as u16;

    // Performance bias: invert (0=high perf = 1000)
    let epb_perf_bias = (1000u32.saturating_sub(epb_hint as u32)).min(1000) as u16;

    // Efficiency bias: direct (15=max efficiency = 1000)
    let epb_efficiency_bias = epb_hint;

    // EMA of raw EPB hint (smoothed signal)
    let mut s = MODULE.lock();
    let epb_ema = ((s.epb_ema as u32).wrapping_mul(7)
        .saturating_add(epb_hint as u32) / 8) as u16;

    s.epb_hint = epb_hint;
    s.epb_perf_bias = epb_perf_bias;
    s.epb_efficiency_bias = epb_efficiency_bias;
    s.epb_ema = epb_ema;

    serial_println!("[msr_ia32_energy_perf_bias] age={} hint={} perf={} eff={} ema={}",
        age, epb_hint, epb_perf_bias, epb_efficiency_bias, epb_ema);
}

pub fn get_epb_hint()            -> u16 { MODULE.lock().epb_hint }
pub fn get_epb_perf_bias()       -> u16 { MODULE.lock().epb_perf_bias }
pub fn get_epb_efficiency_bias() -> u16 { MODULE.lock().epb_efficiency_bias }
pub fn get_epb_ema()             -> u16 { MODULE.lock().epb_ema }

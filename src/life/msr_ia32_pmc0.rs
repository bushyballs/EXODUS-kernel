#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc0_delta: u16, pmc0_rate: u16, pmc0_trend: u16, pmc0_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { pmc0_delta:0, pmc0_rate:0, pmc0_trend:0, pmc0_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_pmc0] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC1u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let pmc0_delta = (delta_lo / 4096).min(1000) as u16;
    let pmc0_rate = pmc0_delta;
    let pmc0_trend: u16 = if pmc0_delta >= s.pmc0_ema { ((pmc0_delta as u32).saturating_sub(s.pmc0_ema as u32)).min(1000) as u16 } else { 0 };
    let pmc0_ema = ((s.pmc0_ema as u32).wrapping_mul(7).saturating_add(pmc0_delta as u32)/8) as u16;
    s.last_lo=lo; s.pmc0_delta=pmc0_delta; s.pmc0_rate=pmc0_rate; s.pmc0_trend=pmc0_trend; s.pmc0_ema=pmc0_ema;
    serial_println!("[msr_ia32_pmc0] age={} delta={} rate={} trend={} ema={}", age, pmc0_delta, pmc0_rate, pmc0_trend, pmc0_ema);
}
pub fn get_pmc0_delta() -> u16 { MODULE.lock().pmc0_delta }
pub fn get_pmc0_rate()  -> u16 { MODULE.lock().pmc0_rate }
pub fn get_pmc0_trend() -> u16 { MODULE.lock().pmc0_trend }
pub fn get_pmc0_ema()   -> u16 { MODULE.lock().pmc0_ema }

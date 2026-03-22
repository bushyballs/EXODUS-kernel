#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc1_delta: u16, pmc1_rate: u16, pmc1_trend: u16, pmc1_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { pmc1_delta:0, pmc1_rate:0, pmc1_trend:0, pmc1_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_pmc1] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC2u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let pmc1_delta = (delta_lo / 4096).min(1000) as u16;
    let pmc1_rate = pmc1_delta;
    let pmc1_trend: u16 = if pmc1_delta >= s.pmc1_ema { ((pmc1_delta as u32).saturating_sub(s.pmc1_ema as u32)).min(1000) as u16 } else { 0 };
    let pmc1_ema = ((s.pmc1_ema as u32).wrapping_mul(7).saturating_add(pmc1_delta as u32)/8) as u16;
    s.last_lo=lo; s.pmc1_delta=pmc1_delta; s.pmc1_rate=pmc1_rate; s.pmc1_trend=pmc1_trend; s.pmc1_ema=pmc1_ema;
    serial_println!("[msr_ia32_pmc1] age={} delta={} rate={} trend={} ema={}", age, pmc1_delta, pmc1_rate, pmc1_trend, pmc1_ema);
}
pub fn get_pmc1_delta() -> u16 { MODULE.lock().pmc1_delta }
pub fn get_pmc1_rate()  -> u16 { MODULE.lock().pmc1_rate }
pub fn get_pmc1_trend() -> u16 { MODULE.lock().pmc1_trend }
pub fn get_pmc1_ema()   -> u16 { MODULE.lock().pmc1_ema }

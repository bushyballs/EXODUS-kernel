#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc3_delta: u16, pmc3_rate: u16, pmc3_trend: u16, pmc3_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { pmc3_delta:0, pmc3_rate:0, pmc3_trend:0, pmc3_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_pmc3] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC4u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let pmc3_delta = (delta_lo / 4096).min(1000) as u16;
    let pmc3_rate = pmc3_delta;
    let pmc3_trend: u16 = if pmc3_delta >= s.pmc3_ema { ((pmc3_delta as u32).saturating_sub(s.pmc3_ema as u32)).min(1000) as u16 } else { 0 };
    let pmc3_ema = ((s.pmc3_ema as u32).wrapping_mul(7).saturating_add(pmc3_delta as u32)/8) as u16;
    s.last_lo=lo; s.pmc3_delta=pmc3_delta; s.pmc3_rate=pmc3_rate; s.pmc3_trend=pmc3_trend; s.pmc3_ema=pmc3_ema;
    serial_println!("[msr_ia32_pmc3] age={} delta={} rate={} trend={} ema={}", age, pmc3_delta, pmc3_rate, pmc3_trend, pmc3_ema);
}
pub fn get_pmc3_delta() -> u16 { MODULE.lock().pmc3_delta }
pub fn get_pmc3_rate()  -> u16 { MODULE.lock().pmc3_rate }
pub fn get_pmc3_trend() -> u16 { MODULE.lock().pmc3_trend }
pub fn get_pmc3_ema()   -> u16 { MODULE.lock().pmc3_ema }

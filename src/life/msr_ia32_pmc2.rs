#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc2_delta: u16, pmc2_rate: u16, pmc2_trend: u16, pmc2_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { pmc2_delta:0, pmc2_rate:0, pmc2_trend:0, pmc2_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_pmc2] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC3u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let pmc2_delta = (delta_lo / 4096).min(1000) as u16;
    let pmc2_rate = pmc2_delta;
    let pmc2_trend: u16 = if pmc2_delta >= s.pmc2_ema { ((pmc2_delta as u32).saturating_sub(s.pmc2_ema as u32)).min(1000) as u16 } else { 0 };
    let pmc2_ema = ((s.pmc2_ema as u32).wrapping_mul(7).saturating_add(pmc2_delta as u32)/8) as u16;
    s.last_lo=lo; s.pmc2_delta=pmc2_delta; s.pmc2_rate=pmc2_rate; s.pmc2_trend=pmc2_trend; s.pmc2_ema=pmc2_ema;
    serial_println!("[msr_ia32_pmc2] age={} delta={} rate={} trend={} ema={}", age, pmc2_delta, pmc2_rate, pmc2_trend, pmc2_ema);
}
pub fn get_pmc2_delta() -> u16 { MODULE.lock().pmc2_delta }
pub fn get_pmc2_rate()  -> u16 { MODULE.lock().pmc2_rate }
pub fn get_pmc2_trend() -> u16 { MODULE.lock().pmc2_trend }
pub fn get_pmc2_ema()   -> u16 { MODULE.lock().pmc2_ema }

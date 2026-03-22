#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { fixed2_delta: u16, fixed2_rate: u16, fixed2_trend: u16, fixed2_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { fixed2_delta:0, fixed2_rate:0, fixed2_trend:0, fixed2_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_fixed_ctr2] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x30Bu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let fixed2_delta = (delta_lo / 4096).min(1000) as u16;
    let fixed2_rate = fixed2_delta;
    let fixed2_trend: u16 = if fixed2_delta >= s.fixed2_ema { ((fixed2_delta as u32).saturating_sub(s.fixed2_ema as u32)).min(1000) as u16 } else { 0 };
    let fixed2_ema = ((s.fixed2_ema as u32).wrapping_mul(7).saturating_add(fixed2_delta as u32)/8) as u16;
    s.last_lo=lo; s.fixed2_delta=fixed2_delta; s.fixed2_rate=fixed2_rate; s.fixed2_trend=fixed2_trend; s.fixed2_ema=fixed2_ema;
    serial_println!("[msr_ia32_fixed_ctr2] age={} delta={} rate={} trend={} ema={}", age, fixed2_delta, fixed2_rate, fixed2_trend, fixed2_ema);
}
pub fn get_fixed2_delta() -> u16 { MODULE.lock().fixed2_delta }
pub fn get_fixed2_rate()  -> u16 { MODULE.lock().fixed2_rate }
pub fn get_fixed2_trend() -> u16 { MODULE.lock().fixed2_trend }
pub fn get_fixed2_ema()   -> u16 { MODULE.lock().fixed2_ema }

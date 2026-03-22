#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { fixed0_delta: u16, fixed0_rate: u16, fixed0_trend: u16, fixed0_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { fixed0_delta:0, fixed0_rate:0, fixed0_trend:0, fixed0_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_fixed_ctr0] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x309u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let fixed0_delta = (delta_lo / 4096).min(1000) as u16;
    let fixed0_rate = fixed0_delta;
    let fixed0_trend: u16 = if fixed0_delta >= s.fixed0_ema { ((fixed0_delta as u32).saturating_sub(s.fixed0_ema as u32)).min(1000) as u16 } else { 0 };
    let fixed0_ema = ((s.fixed0_ema as u32).wrapping_mul(7).saturating_add(fixed0_delta as u32)/8) as u16;
    s.last_lo=lo; s.fixed0_delta=fixed0_delta; s.fixed0_rate=fixed0_rate; s.fixed0_trend=fixed0_trend; s.fixed0_ema=fixed0_ema;
    serial_println!("[msr_ia32_fixed_ctr0] age={} delta={} rate={} trend={} ema={}", age, fixed0_delta, fixed0_rate, fixed0_trend, fixed0_ema);
}
pub fn get_fixed0_delta() -> u16 { MODULE.lock().fixed0_delta }
pub fn get_fixed0_rate()  -> u16 { MODULE.lock().fixed0_rate }
pub fn get_fixed0_trend() -> u16 { MODULE.lock().fixed0_trend }
pub fn get_fixed0_ema()   -> u16 { MODULE.lock().fixed0_ema }

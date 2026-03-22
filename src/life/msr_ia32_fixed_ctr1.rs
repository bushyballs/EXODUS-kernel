#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { fixed1_delta: u16, fixed1_rate: u16, fixed1_trend: u16, fixed1_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { fixed1_delta:0, fixed1_rate:0, fixed1_trend:0, fixed1_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_fixed_ctr1] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x30Au32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let _ = hi;
    let mut s = MODULE.lock();
    let delta_lo = lo.wrapping_sub(s.last_lo);
    let fixed1_delta = (delta_lo / 4096).min(1000) as u16;
    let fixed1_rate = fixed1_delta;
    let fixed1_trend: u16 = if fixed1_delta >= s.fixed1_ema { ((fixed1_delta as u32).saturating_sub(s.fixed1_ema as u32)).min(1000) as u16 } else { 0 };
    let fixed1_ema = ((s.fixed1_ema as u32).wrapping_mul(7).saturating_add(fixed1_delta as u32)/8) as u16;
    s.last_lo=lo; s.fixed1_delta=fixed1_delta; s.fixed1_rate=fixed1_rate; s.fixed1_trend=fixed1_trend; s.fixed1_ema=fixed1_ema;
    serial_println!("[msr_ia32_fixed_ctr1] age={} delta={} rate={} trend={} ema={}", age, fixed1_delta, fixed1_rate, fixed1_trend, fixed1_ema);
}
pub fn get_fixed1_delta() -> u16 { MODULE.lock().fixed1_delta }
pub fn get_fixed1_rate()  -> u16 { MODULE.lock().fixed1_rate }
pub fn get_fixed1_trend() -> u16 { MODULE.lock().fixed1_trend }
pub fn get_fixed1_ema()   -> u16 { MODULE.lock().fixed1_ema }

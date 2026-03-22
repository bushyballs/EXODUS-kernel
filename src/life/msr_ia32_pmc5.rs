#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc5_delta: u16, pmc5_rate: u16, pmc5_trend: u16, pmc5_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pmc5_delta:0, pmc5_rate:0, pmc5_trend:0, pmc5_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pmc5] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x0C6u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.pmc5_delta;
    let pmc5_delta = (lo & 0xFFFF) as u16;
    let raw_rate = (pmc5_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let pmc5_rate = raw_rate;
    let pmc5_trend = if pmc5_rate > prev { 1000u16.min(pmc5_rate) } else { 0 };
    let composite = (pmc5_delta as u32/3).saturating_add(pmc5_rate as u32/3).saturating_add(pmc5_trend as u32/3);
    let pmc5_ema = ((s.pmc5_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pmc5_delta=pmc5_delta; s.pmc5_rate=pmc5_rate; s.pmc5_trend=pmc5_trend; s.pmc5_ema=pmc5_ema;
    serial_println!("[msr_ia32_pmc5] age={} delta={} rate={} trend={} ema={}", age, pmc5_delta, pmc5_rate, pmc5_trend, pmc5_ema);
}
pub fn get_pmc5_delta() -> u16 { MODULE.lock().pmc5_delta }
pub fn get_pmc5_rate()  -> u16 { MODULE.lock().pmc5_rate }
pub fn get_pmc5_trend() -> u16 { MODULE.lock().pmc5_trend }
pub fn get_pmc5_ema()   -> u16 { MODULE.lock().pmc5_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc7_delta: u16, pmc7_rate: u16, pmc7_trend: u16, pmc7_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pmc7_delta:0, pmc7_rate:0, pmc7_trend:0, pmc7_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pmc7] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x0C8u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.pmc7_delta;
    let pmc7_delta = (lo & 0xFFFF) as u16;
    let raw_rate = (pmc7_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let pmc7_rate = raw_rate;
    let pmc7_trend = if pmc7_rate > prev { 1000u16.min(pmc7_rate) } else { 0 };
    let composite = (pmc7_delta as u32/3).saturating_add(pmc7_rate as u32/3).saturating_add(pmc7_trend as u32/3);
    let pmc7_ema = ((s.pmc7_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pmc7_delta=pmc7_delta; s.pmc7_rate=pmc7_rate; s.pmc7_trend=pmc7_trend; s.pmc7_ema=pmc7_ema;
    serial_println!("[msr_ia32_pmc7] age={} delta={} rate={} trend={} ema={}", age, pmc7_delta, pmc7_rate, pmc7_trend, pmc7_ema);
}
pub fn get_pmc7_delta() -> u16 { MODULE.lock().pmc7_delta }
pub fn get_pmc7_rate()  -> u16 { MODULE.lock().pmc7_rate }
pub fn get_pmc7_trend() -> u16 { MODULE.lock().pmc7_trend }
pub fn get_pmc7_ema()   -> u16 { MODULE.lock().pmc7_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc4_delta: u16, pmc4_rate: u16, pmc4_trend: u16, pmc4_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pmc4_delta:0, pmc4_rate:0, pmc4_trend:0, pmc4_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pmc4] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x0C5u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.pmc4_delta;
    let pmc4_delta = (lo & 0xFFFF) as u16;
    let raw_rate = (pmc4_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let pmc4_rate = raw_rate;
    let pmc4_trend = if pmc4_rate > prev { 1000u16.min(pmc4_rate) } else { 0 };
    let composite = (pmc4_delta as u32/3).saturating_add(pmc4_rate as u32/3).saturating_add(pmc4_trend as u32/3);
    let pmc4_ema = ((s.pmc4_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pmc4_delta=pmc4_delta; s.pmc4_rate=pmc4_rate; s.pmc4_trend=pmc4_trend; s.pmc4_ema=pmc4_ema;
    serial_println!("[msr_ia32_pmc4] age={} delta={} rate={} trend={} ema={}", age, pmc4_delta, pmc4_rate, pmc4_trend, pmc4_ema);
}
pub fn get_pmc4_delta() -> u16 { MODULE.lock().pmc4_delta }
pub fn get_pmc4_rate()  -> u16 { MODULE.lock().pmc4_rate }
pub fn get_pmc4_trend() -> u16 { MODULE.lock().pmc4_trend }
pub fn get_pmc4_ema()   -> u16 { MODULE.lock().pmc4_ema }

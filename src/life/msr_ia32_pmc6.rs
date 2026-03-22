#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pmc6_delta: u16, pmc6_rate: u16, pmc6_trend: u16, pmc6_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pmc6_delta:0, pmc6_rate:0, pmc6_trend:0, pmc6_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pmc6] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x0C7u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.pmc6_delta;
    let pmc6_delta = (lo & 0xFFFF) as u16;
    let raw_rate = (pmc6_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let pmc6_rate = raw_rate;
    let pmc6_trend = if pmc6_rate > prev { 1000u16.min(pmc6_rate) } else { 0 };
    let composite = (pmc6_delta as u32/3).saturating_add(pmc6_rate as u32/3).saturating_add(pmc6_trend as u32/3);
    let pmc6_ema = ((s.pmc6_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pmc6_delta=pmc6_delta; s.pmc6_rate=pmc6_rate; s.pmc6_trend=pmc6_trend; s.pmc6_ema=pmc6_ema;
    serial_println!("[msr_ia32_pmc6] age={} delta={} rate={} trend={} ema={}", age, pmc6_delta, pmc6_rate, pmc6_trend, pmc6_ema);
}
pub fn get_pmc6_delta() -> u16 { MODULE.lock().pmc6_delta }
pub fn get_pmc6_rate()  -> u16 { MODULE.lock().pmc6_rate }
pub fn get_pmc6_trend() -> u16 { MODULE.lock().pmc6_trend }
pub fn get_pmc6_ema()   -> u16 { MODULE.lock().pmc6_ema }

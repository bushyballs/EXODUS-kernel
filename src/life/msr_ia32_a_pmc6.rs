#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc6_delta: u16, apmc6_rate: u16, apmc6_nonzero: u16, apmc6_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc6_delta:0, apmc6_rate:0, apmc6_nonzero:0, apmc6_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc6] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C7u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc6_delta;
    let apmc6_delta = (lo & 0xFFFF) as u16;
    let apmc6_rate = (apmc6_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc6_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc6_delta as u32/3).saturating_add(apmc6_rate as u32/3).saturating_add(apmc6_nonzero as u32/3);
    let apmc6_ema = ((s.apmc6_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc6_delta=apmc6_delta; s.apmc6_rate=apmc6_rate; s.apmc6_nonzero=apmc6_nonzero; s.apmc6_ema=apmc6_ema;
    serial_println!("[msr_ia32_a_pmc6] age={} delta={} rate={} nz={} ema={}", age, apmc6_delta, apmc6_rate, apmc6_nonzero, apmc6_ema);
}
pub fn get_apmc6_delta()   -> u16 { MODULE.lock().apmc6_delta }
pub fn get_apmc6_rate()    -> u16 { MODULE.lock().apmc6_rate }
pub fn get_apmc6_nonzero() -> u16 { MODULE.lock().apmc6_nonzero }
pub fn get_apmc6_ema()     -> u16 { MODULE.lock().apmc6_ema }

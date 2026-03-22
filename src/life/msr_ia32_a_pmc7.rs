#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc7_delta: u16, apmc7_rate: u16, apmc7_nonzero: u16, apmc7_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc7_delta:0, apmc7_rate:0, apmc7_nonzero:0, apmc7_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc7] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C8u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc7_delta;
    let apmc7_delta = (lo & 0xFFFF) as u16;
    let apmc7_rate = (apmc7_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc7_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc7_delta as u32/3).saturating_add(apmc7_rate as u32/3).saturating_add(apmc7_nonzero as u32/3);
    let apmc7_ema = ((s.apmc7_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc7_delta=apmc7_delta; s.apmc7_rate=apmc7_rate; s.apmc7_nonzero=apmc7_nonzero; s.apmc7_ema=apmc7_ema;
    serial_println!("[msr_ia32_a_pmc7] age={} delta={} rate={} nz={} ema={}", age, apmc7_delta, apmc7_rate, apmc7_nonzero, apmc7_ema);
}
pub fn get_apmc7_delta()   -> u16 { MODULE.lock().apmc7_delta }
pub fn get_apmc7_rate()    -> u16 { MODULE.lock().apmc7_rate }
pub fn get_apmc7_nonzero() -> u16 { MODULE.lock().apmc7_nonzero }
pub fn get_apmc7_ema()     -> u16 { MODULE.lock().apmc7_ema }

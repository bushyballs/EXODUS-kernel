#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc4_delta: u16, apmc4_rate: u16, apmc4_nonzero: u16, apmc4_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc4_delta:0, apmc4_rate:0, apmc4_nonzero:0, apmc4_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc4] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C5u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc4_delta;
    let apmc4_delta = (lo & 0xFFFF) as u16;
    let apmc4_rate = (apmc4_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc4_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc4_delta as u32/3).saturating_add(apmc4_rate as u32/3).saturating_add(apmc4_nonzero as u32/3);
    let apmc4_ema = ((s.apmc4_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc4_delta=apmc4_delta; s.apmc4_rate=apmc4_rate; s.apmc4_nonzero=apmc4_nonzero; s.apmc4_ema=apmc4_ema;
    serial_println!("[msr_ia32_a_pmc4] age={} delta={} rate={} nz={} ema={}", age, apmc4_delta, apmc4_rate, apmc4_nonzero, apmc4_ema);
}
pub fn get_apmc4_delta()   -> u16 { MODULE.lock().apmc4_delta }
pub fn get_apmc4_rate()    -> u16 { MODULE.lock().apmc4_rate }
pub fn get_apmc4_nonzero() -> u16 { MODULE.lock().apmc4_nonzero }
pub fn get_apmc4_ema()     -> u16 { MODULE.lock().apmc4_ema }

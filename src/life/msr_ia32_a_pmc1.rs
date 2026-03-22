#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc1_delta: u16, apmc1_rate: u16, apmc1_nonzero: u16, apmc1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc1_delta:0, apmc1_rate:0, apmc1_nonzero:0, apmc1_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc1] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C2u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc1_delta;
    let apmc1_delta = (lo & 0xFFFF) as u16;
    let apmc1_rate = (apmc1_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc1_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc1_delta as u32/3).saturating_add(apmc1_rate as u32/3).saturating_add(apmc1_nonzero as u32/3);
    let apmc1_ema = ((s.apmc1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc1_delta=apmc1_delta; s.apmc1_rate=apmc1_rate; s.apmc1_nonzero=apmc1_nonzero; s.apmc1_ema=apmc1_ema;
    serial_println!("[msr_ia32_a_pmc1] age={} delta={} rate={} nz={} ema={}", age, apmc1_delta, apmc1_rate, apmc1_nonzero, apmc1_ema);
}
pub fn get_apmc1_delta()   -> u16 { MODULE.lock().apmc1_delta }
pub fn get_apmc1_rate()    -> u16 { MODULE.lock().apmc1_rate }
pub fn get_apmc1_nonzero() -> u16 { MODULE.lock().apmc1_nonzero }
pub fn get_apmc1_ema()     -> u16 { MODULE.lock().apmc1_ema }

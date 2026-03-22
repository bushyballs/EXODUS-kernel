#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc5_delta: u16, apmc5_rate: u16, apmc5_nonzero: u16, apmc5_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc5_delta:0, apmc5_rate:0, apmc5_nonzero:0, apmc5_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc5] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C6u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc5_delta;
    let apmc5_delta = (lo & 0xFFFF) as u16;
    let apmc5_rate = (apmc5_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc5_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc5_delta as u32/3).saturating_add(apmc5_rate as u32/3).saturating_add(apmc5_nonzero as u32/3);
    let apmc5_ema = ((s.apmc5_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc5_delta=apmc5_delta; s.apmc5_rate=apmc5_rate; s.apmc5_nonzero=apmc5_nonzero; s.apmc5_ema=apmc5_ema;
    serial_println!("[msr_ia32_a_pmc5] age={} delta={} rate={} nz={} ema={}", age, apmc5_delta, apmc5_rate, apmc5_nonzero, apmc5_ema);
}
pub fn get_apmc5_delta()   -> u16 { MODULE.lock().apmc5_delta }
pub fn get_apmc5_rate()    -> u16 { MODULE.lock().apmc5_rate }
pub fn get_apmc5_nonzero() -> u16 { MODULE.lock().apmc5_nonzero }
pub fn get_apmc5_ema()     -> u16 { MODULE.lock().apmc5_ema }

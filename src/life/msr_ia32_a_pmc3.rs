#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc3_delta: u16, apmc3_rate: u16, apmc3_nonzero: u16, apmc3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc3_delta:0, apmc3_rate:0, apmc3_nonzero:0, apmc3_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc3] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C4u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc3_delta;
    let apmc3_delta = (lo & 0xFFFF) as u16;
    let apmc3_rate = (apmc3_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc3_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc3_delta as u32/3).saturating_add(apmc3_rate as u32/3).saturating_add(apmc3_nonzero as u32/3);
    let apmc3_ema = ((s.apmc3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc3_delta=apmc3_delta; s.apmc3_rate=apmc3_rate; s.apmc3_nonzero=apmc3_nonzero; s.apmc3_ema=apmc3_ema;
    serial_println!("[msr_ia32_a_pmc3] age={} delta={} rate={} nz={} ema={}", age, apmc3_delta, apmc3_rate, apmc3_nonzero, apmc3_ema);
}
pub fn get_apmc3_delta()   -> u16 { MODULE.lock().apmc3_delta }
pub fn get_apmc3_rate()    -> u16 { MODULE.lock().apmc3_rate }
pub fn get_apmc3_nonzero() -> u16 { MODULE.lock().apmc3_nonzero }
pub fn get_apmc3_ema()     -> u16 { MODULE.lock().apmc3_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc0_delta: u16, apmc0_rate: u16, apmc0_nonzero: u16, apmc0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc0_delta:0, apmc0_rate:0, apmc0_nonzero:0, apmc0_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc0] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C1u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc0_delta;
    let apmc0_delta = (lo & 0xFFFF) as u16;
    let apmc0_rate = (apmc0_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc0_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc0_delta as u32/3).saturating_add(apmc0_rate as u32/3).saturating_add(apmc0_nonzero as u32/3);
    let apmc0_ema = ((s.apmc0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc0_delta=apmc0_delta; s.apmc0_rate=apmc0_rate; s.apmc0_nonzero=apmc0_nonzero; s.apmc0_ema=apmc0_ema;
    serial_println!("[msr_ia32_a_pmc0] age={} delta={} rate={} nz={} ema={}", age, apmc0_delta, apmc0_rate, apmc0_nonzero, apmc0_ema);
}
pub fn get_apmc0_delta()   -> u16 { MODULE.lock().apmc0_delta }
pub fn get_apmc0_rate()    -> u16 { MODULE.lock().apmc0_rate }
pub fn get_apmc0_nonzero() -> u16 { MODULE.lock().apmc0_nonzero }
pub fn get_apmc0_ema()     -> u16 { MODULE.lock().apmc0_ema }

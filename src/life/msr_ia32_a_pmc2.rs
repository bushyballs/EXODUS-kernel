#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { apmc2_delta: u16, apmc2_rate: u16, apmc2_nonzero: u16, apmc2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { apmc2_delta:0, apmc2_rate:0, apmc2_nonzero:0, apmc2_ema:0 });
pub fn init() { serial_println!("[msr_ia32_a_pmc2] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4C3u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mut s = MODULE.lock();
    let prev = s.apmc2_delta;
    let apmc2_delta = (lo & 0xFFFF) as u16;
    let apmc2_rate = (apmc2_delta as u32).saturating_sub(prev as u32).min(1000) as u16;
    let apmc2_nonzero: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (apmc2_delta as u32/3).saturating_add(apmc2_rate as u32/3).saturating_add(apmc2_nonzero as u32/3);
    let apmc2_ema = ((s.apmc2_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.apmc2_delta=apmc2_delta; s.apmc2_rate=apmc2_rate; s.apmc2_nonzero=apmc2_nonzero; s.apmc2_ema=apmc2_ema;
    serial_println!("[msr_ia32_a_pmc2] age={} delta={} rate={} nz={} ema={}", age, apmc2_delta, apmc2_rate, apmc2_nonzero, apmc2_ema);
}
pub fn get_apmc2_delta()   -> u16 { MODULE.lock().apmc2_delta }
pub fn get_apmc2_rate()    -> u16 { MODULE.lock().apmc2_rate }
pub fn get_apmc2_nonzero() -> u16 { MODULE.lock().apmc2_nonzero }
pub fn get_apmc2_ema()     -> u16 { MODULE.lock().apmc2_ema }

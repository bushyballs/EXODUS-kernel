#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { xss_pt: u16, xss_cet_u: u16, xss_cet_s: u16, xss_density_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { xss_pt:0, xss_cet_u:0, xss_cet_s:0, xss_density_ema:0 });

pub fn init() { serial_println!("[msr_ia32_xss] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xDA0u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 8: PT (Processor Trace) state in supervisor XSAVE
    let xss_pt: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    // bit 11: CET_U user shadow stack state
    let xss_cet_u: u16 = if (lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // bit 12: CET_S supervisor shadow stack state
    let xss_cet_s: u16 = if (lo >> 12) & 1 != 0 { 1000 } else { 0 };
    let composite = (xss_pt as u32/3).saturating_add(xss_cet_u as u32/3).saturating_add(xss_cet_s as u32/3);
    let mut s = MODULE.lock();
    let xss_density_ema = ((s.xss_density_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.xss_pt=xss_pt; s.xss_cet_u=xss_cet_u; s.xss_cet_s=xss_cet_s; s.xss_density_ema=xss_density_ema;
    serial_println!("[msr_ia32_xss] age={} pt={} cet_u={} cet_s={} ema={}", age, xss_pt, xss_cet_u, xss_cet_s, xss_density_ema);
}
pub fn get_xss_pt()           -> u16 { MODULE.lock().xss_pt }
pub fn get_xss_cet_u()        -> u16 { MODULE.lock().xss_cet_u }
pub fn get_xss_cet_s()        -> u16 { MODULE.lock().xss_cet_s }
pub fn get_xss_density_ema()  -> u16 { MODULE.lock().xss_density_ema }

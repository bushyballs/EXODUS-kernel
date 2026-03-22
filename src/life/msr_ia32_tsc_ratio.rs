#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tsc_ratio_lo: u16, tsc_ratio_hi: u16, tsc_scale: u16, tsc_ratio_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tsc_ratio_lo:0, tsc_ratio_hi:0, tsc_scale:0, tsc_ratio_ema:0 });

pub fn init() { serial_println!("[msr_ia32_tsc_ratio] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x64Fu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // Ratio of TSC to core crystal clock: Q16.48 fixed-point
    // lo[15:0] = fractional part sense, lo[31:16] = integer low
    let tsc_ratio_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let tsc_ratio_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Integer part of ratio (lo[31:16]) — 0=default (1:1)
    let int_ratio = (lo >> 16) & 0xFFFF;
    let tsc_scale = if int_ratio == 0 { 500u16 } else { (int_ratio.min(1000)) as u16 };
    let composite = (tsc_ratio_lo as u32/3).saturating_add(tsc_ratio_hi as u32/3).saturating_add(tsc_scale as u32/3);
    let mut s = MODULE.lock();
    let tsc_ratio_ema = ((s.tsc_ratio_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.tsc_ratio_lo=tsc_ratio_lo; s.tsc_ratio_hi=tsc_ratio_hi; s.tsc_scale=tsc_scale; s.tsc_ratio_ema=tsc_ratio_ema;
    serial_println!("[msr_ia32_tsc_ratio] age={} lo={} hi={} scale={} ema={}", age, tsc_ratio_lo, tsc_ratio_hi, tsc_scale, tsc_ratio_ema);
}
pub fn get_tsc_ratio_lo()  -> u16 { MODULE.lock().tsc_ratio_lo }
pub fn get_tsc_ratio_hi()  -> u16 { MODULE.lock().tsc_ratio_hi }
pub fn get_tsc_scale()     -> u16 { MODULE.lock().tsc_scale }
pub fn get_tsc_ratio_ema() -> u16 { MODULE.lock().tsc_ratio_ema }

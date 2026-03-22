#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { dca_cap_value: u16, dca_supported: u16, dca_platform_bits: u16, dca_leaf_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { dca_cap_value: 0, dca_supported: 0, dca_platform_bits: 0, dca_leaf_ema: 0 });

fn popcount(mut v: u32) -> u32 { let mut c=0; while v!=0{c+=v&1;v>>=1;} c }

pub fn init() { serial_println!("[cpuid_dca_leaf] init"); }
pub fn tick(age: u32) {
    if age % 6000 != 0 { return; }
    let max: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max < 9 { return; }
    let eax_out: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 9u32 => eax_out, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
    let cap_raw = eax_out & 0x1FFFF;
    let dca_cap_value = ((cap_raw as u32 * 1000) / 0x1FFFF).min(1000) as u16;
    let dca_supported: u16 = if eax_out != 0 { 1000 } else { 0 };
    let bits = popcount(eax_out & 0xFFFF);
    let dca_platform_bits = (bits * 62).min(1000) as u16;
    let composite: u16 = (dca_cap_value/4).saturating_add(dca_supported/4).saturating_add(dca_platform_bits/2);
    let mut s = MODULE.lock();
    let ema = ((s.dca_leaf_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.dca_cap_value = dca_cap_value; s.dca_supported = dca_supported; s.dca_platform_bits = dca_platform_bits; s.dca_leaf_ema = ema;
    serial_println!("[cpuid_dca_leaf] age={} cap={} supp={} bits={} ema={}", age, dca_cap_value, dca_supported, dca_platform_bits, ema);
}
pub fn get_dca_cap_value() -> u16 { MODULE.lock().dca_cap_value }
pub fn get_dca_supported() -> u16 { MODULE.lock().dca_supported }
pub fn get_dca_platform_bits() -> u16 { MODULE.lock().dca_platform_bits }
pub fn get_dca_leaf_ema() -> u16 { MODULE.lock().dca_leaf_ema }

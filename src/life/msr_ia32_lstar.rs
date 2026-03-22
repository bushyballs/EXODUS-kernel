#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lstar_set: u16, lstar_kernel_space: u16, lstar_canonical: u16, lstar_health_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lstar_set:0, lstar_kernel_space:0, lstar_canonical:0, lstar_health_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lstar] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000082u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lstar_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let lstar_kernel_space: u16 = if (hi >> 16) == 0xFFFF { 1000 } else { 0 };
    let hi_top = (hi >> 16) & 0xFFFF;
    let lstar_canonical: u16 = if hi_top == 0xFFFF || hi_top == 0x0000 { 1000 } else { 0 };
    let composite = (lstar_set as u32/3).saturating_add(lstar_kernel_space as u32/3).saturating_add(lstar_canonical as u32/3);
    let mut s = MODULE.lock();
    let lstar_health_ema = ((s.lstar_health_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lstar_set=lstar_set; s.lstar_kernel_space=lstar_kernel_space; s.lstar_canonical=lstar_canonical; s.lstar_health_ema=lstar_health_ema;
    serial_println!("[msr_ia32_lstar] age={} set={} kspace={} canonical={} ema={}", age, lstar_set, lstar_kernel_space, lstar_canonical, lstar_health_ema);
}
pub fn get_lstar_set()          -> u16 { MODULE.lock().lstar_set }
pub fn get_lstar_kernel_space() -> u16 { MODULE.lock().lstar_kernel_space }
pub fn get_lstar_canonical()    -> u16 { MODULE.lock().lstar_canonical }
pub fn get_lstar_health_ema()   -> u16 { MODULE.lock().lstar_health_ema }

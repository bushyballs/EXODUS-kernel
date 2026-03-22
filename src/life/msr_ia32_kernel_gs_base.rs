#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { kgs_set: u16, kgs_kernel_space: u16, kgs_canonical: u16, kgs_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { kgs_set:0, kgs_kernel_space:0, kgs_canonical:0, kgs_ema:0 });

pub fn init() { serial_println!("[msr_ia32_kernel_gs_base] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000102u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let kgs_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let kgs_kernel_space: u16 = if (hi >> 16) == 0xFFFF { 1000 } else { 0 };
    let hi_top = (hi >> 16) & 0xFFFF;
    let kgs_canonical: u16 = if hi_top == 0xFFFF || hi_top == 0x0000 { 1000 } else { 0 };
    let composite = (kgs_set as u32/3).saturating_add(kgs_kernel_space as u32/3).saturating_add(kgs_canonical as u32/3);
    let mut s = MODULE.lock();
    let kgs_ema = ((s.kgs_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.kgs_set=kgs_set; s.kgs_kernel_space=kgs_kernel_space; s.kgs_canonical=kgs_canonical; s.kgs_ema=kgs_ema;
    serial_println!("[msr_ia32_kernel_gs_base] age={} set={} kspace={} canonical={} ema={}", age, kgs_set, kgs_kernel_space, kgs_canonical, kgs_ema);
}
pub fn get_kgs_set()          -> u16 { MODULE.lock().kgs_set }
pub fn get_kgs_kernel_space() -> u16 { MODULE.lock().kgs_kernel_space }
pub fn get_kgs_canonical()    -> u16 { MODULE.lock().kgs_canonical }
pub fn get_kgs_ema()          -> u16 { MODULE.lock().kgs_ema }

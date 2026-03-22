#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { gs_base_set: u16, gs_kernel_space: u16, gs_canonical: u16, gs_base_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { gs_base_set:0, gs_kernel_space:0, gs_canonical:0, gs_base_ema:0 });

pub fn init() { serial_println!("[msr_ia32_gs_base] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000101u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let gs_base_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let gs_kernel_space: u16 = if (hi >> 16) == 0xFFFF { 1000 } else { 0 };
    let hi_top = (hi >> 16) & 0xFFFF;
    let gs_canonical: u16 = if hi_top == 0xFFFF || hi_top == 0x0000 { 1000 } else { 0 };
    let composite = (gs_base_set as u32/3).saturating_add(gs_kernel_space as u32/3).saturating_add(gs_canonical as u32/3);
    let mut s = MODULE.lock();
    let gs_base_ema = ((s.gs_base_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.gs_base_set=gs_base_set; s.gs_kernel_space=gs_kernel_space; s.gs_canonical=gs_canonical; s.gs_base_ema=gs_base_ema;
    serial_println!("[msr_ia32_gs_base] age={} set={} kspace={} canonical={} ema={}", age, gs_base_set, gs_kernel_space, gs_canonical, gs_base_ema);
}
pub fn get_gs_base_set()     -> u16 { MODULE.lock().gs_base_set }
pub fn get_gs_kernel_space() -> u16 { MODULE.lock().gs_kernel_space }
pub fn get_gs_canonical()    -> u16 { MODULE.lock().gs_canonical }
pub fn get_gs_base_ema()     -> u16 { MODULE.lock().gs_base_ema }

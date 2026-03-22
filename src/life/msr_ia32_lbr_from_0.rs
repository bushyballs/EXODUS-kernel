#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_from_valid: u16, lbr_kernel_activity: u16, lbr_user_activity: u16, lbr_from_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_from_valid:0, lbr_kernel_activity:0, lbr_user_activity:0, lbr_from_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lbr_from_0] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x680u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_from_valid: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let hi_top = (hi >> 16) & 0xFFFF;
    let lbr_kernel_activity: u16 = if hi_top == 0xFFFF { 1000 } else { 0 };
    let lbr_user_activity: u16 = if hi_top == 0x0000 && lbr_from_valid != 0 { 1000 } else { 0 };
    let composite = (lbr_from_valid as u32/3).saturating_add(lbr_kernel_activity as u32/3).saturating_add(lbr_user_activity as u32/3);
    let mut s = MODULE.lock();
    let lbr_from_ema = ((s.lbr_from_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_from_valid=lbr_from_valid; s.lbr_kernel_activity=lbr_kernel_activity; s.lbr_user_activity=lbr_user_activity; s.lbr_from_ema=lbr_from_ema;
    serial_println!("[msr_ia32_lbr_from_0] age={} valid={} kern={} user={} ema={}", age, lbr_from_valid, lbr_kernel_activity, lbr_user_activity, lbr_from_ema);
}
pub fn get_lbr_from_valid()      -> u16 { MODULE.lock().lbr_from_valid }
pub fn get_lbr_kernel_activity() -> u16 { MODULE.lock().lbr_kernel_activity }
pub fn get_lbr_user_activity()   -> u16 { MODULE.lock().lbr_user_activity }
pub fn get_lbr_from_ema()        -> u16 { MODULE.lock().lbr_from_ema }

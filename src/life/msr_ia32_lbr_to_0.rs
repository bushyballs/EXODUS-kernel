#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_to_valid: u16, lbr_to_kernel: u16, lbr_to_user: u16, lbr_to_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_to_valid:0, lbr_to_kernel:0, lbr_to_user:0, lbr_to_ema:0 });

pub fn init() { serial_println!("[msr_ia32_lbr_to_0] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6C0u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let lbr_to_valid: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let hi_top = (hi >> 16) & 0xFFFF;
    let lbr_to_kernel: u16 = if hi_top == 0xFFFF { 1000 } else { 0 };
    let lbr_to_user: u16 = if hi_top == 0x0000 && lbr_to_valid != 0 { 1000 } else { 0 };
    let composite = (lbr_to_valid as u32/3).saturating_add(lbr_to_kernel as u32/3).saturating_add(lbr_to_user as u32/3);
    let mut s = MODULE.lock();
    let lbr_to_ema = ((s.lbr_to_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lbr_to_valid=lbr_to_valid; s.lbr_to_kernel=lbr_to_kernel; s.lbr_to_user=lbr_to_user; s.lbr_to_ema=lbr_to_ema;
    serial_println!("[msr_ia32_lbr_to_0] age={} valid={} kern={} user={} ema={}", age, lbr_to_valid, lbr_to_kernel, lbr_to_user, lbr_to_ema);
}
pub fn get_lbr_to_valid()  -> u16 { MODULE.lock().lbr_to_valid }
pub fn get_lbr_to_kernel() -> u16 { MODULE.lock().lbr_to_kernel }
pub fn get_lbr_to_user()   -> u16 { MODULE.lock().lbr_to_user }
pub fn get_lbr_to_ema()    -> u16 { MODULE.lock().lbr_to_ema }

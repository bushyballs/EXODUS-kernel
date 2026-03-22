#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mce_valid: u16, mce_error_flag: u16, mce_overflow: u16, mce_health_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mce_valid:0, mce_error_flag:0, mce_overflow:0, mce_health_ema:0 });

#[inline]
fn has_mce() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mc0_status] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_mce() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x401u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let mce_valid: u16     = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let mce_overflow: u16  = if (hi >> 30) & 1 != 0 { 1000 } else { 0 };
    let mce_error_flag: u16 = if lo != 0 { 1000 } else { 0 };
    let health = 1000u32.saturating_sub(mce_valid as u32/3).saturating_sub(mce_overflow as u32/3).saturating_sub(mce_error_flag as u32/3);
    let mut s = MODULE.lock();
    let mce_health_ema = ((s.mce_health_ema as u32).wrapping_mul(7).saturating_add(health)/8).min(1000) as u16;
    s.mce_valid=mce_valid; s.mce_error_flag=mce_error_flag; s.mce_overflow=mce_overflow; s.mce_health_ema=mce_health_ema;
    serial_println!("[msr_ia32_mc0_status] age={} valid={} err={} overflow={} ema={}", age, mce_valid, mce_error_flag, mce_overflow, mce_health_ema);
}
pub fn get_mce_valid()       -> u16 { MODULE.lock().mce_valid }
pub fn get_mce_error_flag()  -> u16 { MODULE.lock().mce_error_flag }
pub fn get_mce_overflow()    -> u16 { MODULE.lock().mce_overflow }
pub fn get_mce_health_ema()  -> u16 { MODULE.lock().mce_health_ema }

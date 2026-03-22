#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ppin_lock: u16, ppin_en: u16, ppin_ctl_ema: u16, ppin_pad: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ppin_lock:0, ppin_en:0, ppin_ctl_ema:0, ppin_pad:0 });

pub fn init() { serial_println!("[msr_ia32_ppin_ctl] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4Eu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: LockOut — PPIN read/enable locked
    let ppin_lock: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: Enable — PPIN readable (must be 1 to read 0x4F)
    let ppin_en: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let composite = (ppin_lock as u32/2).saturating_add(ppin_en as u32/2);
    let mut s = MODULE.lock();
    let ppin_ctl_ema = ((s.ppin_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ppin_lock=ppin_lock; s.ppin_en=ppin_en; s.ppin_ctl_ema=ppin_ctl_ema;
    serial_println!("[msr_ia32_ppin_ctl] age={} lock={} en={} ema={}", age, ppin_lock, ppin_en, ppin_ctl_ema);
}
pub fn get_ppin_lock()    -> u16 { MODULE.lock().ppin_lock }
pub fn get_ppin_en()      -> u16 { MODULE.lock().ppin_en }
pub fn get_ppin_ctl_ema() -> u16 { MODULE.lock().ppin_ctl_ema }

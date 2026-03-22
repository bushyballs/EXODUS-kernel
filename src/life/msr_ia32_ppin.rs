#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ppin_lo: u16, ppin_hi: u16, ppin_present: u16, ppin_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ppin_lo:0, ppin_hi:0, ppin_present:0, ppin_ema:0 });

pub fn init() { serial_println!("[msr_ia32_ppin] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    // Only readable when PPIN_CTL.Enable=1 (bit 1 of 0x4E)
    let ctl: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4Eu32, out("eax") ctl, out("edx") _, options(nostack, nomem)); }
    if (ctl >> 1) & 1 == 0 { return; }
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x4Fu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // PPIN is a 64-bit processor serial identifier
    let ppin_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let ppin_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let ppin_present: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (ppin_lo as u32/3).saturating_add(ppin_hi as u32/3).saturating_add(ppin_present as u32/3);
    let mut s = MODULE.lock();
    let ppin_ema = ((s.ppin_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ppin_lo=ppin_lo; s.ppin_hi=ppin_hi; s.ppin_present=ppin_present; s.ppin_ema=ppin_ema;
    serial_println!("[msr_ia32_ppin] age={} lo={} hi={} present={} ema={}", age, ppin_lo, ppin_hi, ppin_present, ppin_ema);
}
pub fn get_ppin_lo()      -> u16 { MODULE.lock().ppin_lo }
pub fn get_ppin_hi()      -> u16 { MODULE.lock().ppin_hi }
pub fn get_ppin_present() -> u16 { MODULE.lock().ppin_present }
pub fn get_ppin_ema()     -> u16 { MODULE.lock().ppin_ema }

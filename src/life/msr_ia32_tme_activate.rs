#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tme_enable: u16, tme_locked: u16, tme_enc_bypass: u16, tme_act_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tme_enable: 0, tme_locked: 0, tme_enc_bypass: 0, tme_act_ema: 0 });

fn has_tme() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 7u32 => _, inout("ecx") 0u32 => ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 13) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_tme_activate] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_tme() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x982u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let tme_locked: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let tme_enable: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let tme_enc_bypass: u16 = if (lo >> 31) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (tme_enable/4).saturating_add(tme_locked/4).saturating_add(tme_enc_bypass/2);
    let mut s = MODULE.lock();
    let ema = ((s.tme_act_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.tme_enable = tme_enable; s.tme_locked = tme_locked; s.tme_enc_bypass = tme_enc_bypass; s.tme_act_ema = ema;
    serial_println!("[msr_ia32_tme_activate] age={} lo={:#010x} en={} lock={} bypass={} ema={}", age, lo, tme_enable, tme_locked, tme_enc_bypass, ema);
}
pub fn get_tme_enable() -> u16 { MODULE.lock().tme_enable }
pub fn get_tme_locked() -> u16 { MODULE.lock().tme_locked }
pub fn get_tme_enc_bypass() -> u16 { MODULE.lock().tme_enc_bypass }
pub fn get_tme_act_ema() -> u16 { MODULE.lock().tme_act_ema }

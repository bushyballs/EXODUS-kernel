#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tme_aes_xts_128: u16, tme_aes_xts_256: u16, tme_mk_tme: u16, tme_cap_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tme_aes_xts_128: 0, tme_aes_xts_256: 0, tme_mk_tme: 0, tme_cap_ema: 0 });

fn has_tme() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 7u32 => _, inout("ecx") 0u32 => ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 13) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_tme_capability] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_tme() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x981u32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let tme_aes_xts_128: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let tme_mk_tme: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let tme_aes_xts_256: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (tme_aes_xts_128/4).saturating_add(tme_mk_tme/4).saturating_add(tme_aes_xts_256/2);
    let mut s = MODULE.lock();
    let ema = ((s.tme_cap_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.tme_aes_xts_128 = tme_aes_xts_128; s.tme_aes_xts_256 = tme_aes_xts_256; s.tme_mk_tme = tme_mk_tme; s.tme_cap_ema = ema;
    serial_println!("[msr_ia32_tme_capability] age={} lo={:#010x} aes128={} aes256={} mktme={} ema={}", age, lo, tme_aes_xts_128, tme_aes_xts_256, tme_mk_tme, ema);
}
pub fn get_tme_aes_xts_128() -> u16 { MODULE.lock().tme_aes_xts_128 }
pub fn get_tme_aes_xts_256() -> u16 { MODULE.lock().tme_aes_xts_256 }
pub fn get_tme_mk_tme() -> u16 { MODULE.lock().tme_mk_tme }
pub fn get_tme_cap_ema() -> u16 { MODULE.lock().tme_cap_ema }

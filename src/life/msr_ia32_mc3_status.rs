#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc3_valid: u16, mc3_uncorrectable: u16, mc3_pcc: u16, mc3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc3_valid: 0, mc3_uncorrectable: 0, mc3_pcc: 0, mc3_ema: 0 });

fn has_mca() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 14) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_mc3_status] init"); }
pub fn tick(age: u32) {
    if age % 4500 != 0 { return; }
    if !has_mca() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x40Du32, out("eax") lo, out("edx") hi, options(nostack,nomem)); }
    let mc3_valid: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let mc3_uncorrectable: u16 = if (hi >> 29) & 1 != 0 { 1000 } else { 0 };
    let mc3_pcc: u16 = if (hi >> 25) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (mc3_valid/4).saturating_add(mc3_uncorrectable/4).saturating_add(mc3_pcc/2);
    let mut s = MODULE.lock();
    let ema = ((s.mc3_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.mc3_valid = mc3_valid; s.mc3_uncorrectable = mc3_uncorrectable; s.mc3_pcc = mc3_pcc; s.mc3_ema = ema;
    serial_println!("[msr_ia32_mc3_status] age={} lo={:#010x} hi={:#010x} valid={} unc={} pcc={} ema={}", age, lo, hi, mc3_valid, mc3_uncorrectable, mc3_pcc, ema);
}
pub fn get_mc3_valid() -> u16 { MODULE.lock().mc3_valid }
pub fn get_mc3_uncorrectable() -> u16 { MODULE.lock().mc3_uncorrectable }
pub fn get_mc3_pcc() -> u16 { MODULE.lock().mc3_pcc }
pub fn get_mc3_ema() -> u16 { MODULE.lock().mc3_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc7_valid: u16, mc7_uncorrectable: u16, mc7_pcc: u16, mc7_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc7_valid: 0, mc7_uncorrectable: 0, mc7_pcc: 0, mc7_ema: 0 });

fn has_mca() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 14) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_mc7_status] init"); }
pub fn tick(age: u32) {
    if age % 4500 != 0 { return; }
    if !has_mca() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x41Du32, out("eax") lo, out("edx") hi, options(nostack,nomem)); }
    let mc7_valid: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let mc7_uncorrectable: u16 = if (hi >> 29) & 1 != 0 { 1000 } else { 0 };
    let mc7_pcc: u16 = if (hi >> 25) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (mc7_valid/4).saturating_add(mc7_uncorrectable/4).saturating_add(mc7_pcc/2);
    let mut s = MODULE.lock();
    let ema = ((s.mc7_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.mc7_valid = mc7_valid; s.mc7_uncorrectable = mc7_uncorrectable; s.mc7_pcc = mc7_pcc; s.mc7_ema = ema;
    serial_println!("[msr_ia32_mc7_status] age={} lo={:#010x} hi={:#010x} valid={} unc={} pcc={} ema={}", age, lo, hi, mc7_valid, mc7_uncorrectable, mc7_pcc, ema);
}
pub fn get_mc7_valid() -> u16 { MODULE.lock().mc7_valid }
pub fn get_mc7_uncorrectable() -> u16 { MODULE.lock().mc7_uncorrectable }
pub fn get_mc7_pcc() -> u16 { MODULE.lock().mc7_pcc }
pub fn get_mc7_ema() -> u16 { MODULE.lock().mc7_ema }

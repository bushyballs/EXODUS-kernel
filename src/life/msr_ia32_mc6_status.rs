#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc6_valid: u16, mc6_uncorrectable: u16, mc6_pcc: u16, mc6_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc6_valid: 0, mc6_uncorrectable: 0, mc6_pcc: 0, mc6_ema: 0 });

fn has_mca() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 14) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_mc6_status] init"); }
pub fn tick(age: u32) {
    if age % 4500 != 0 { return; }
    if !has_mca() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x419u32, out("eax") lo, out("edx") hi, options(nostack,nomem)); }
    let mc6_valid: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let mc6_uncorrectable: u16 = if (hi >> 29) & 1 != 0 { 1000 } else { 0 };
    let mc6_pcc: u16 = if (hi >> 25) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (mc6_valid/4).saturating_add(mc6_uncorrectable/4).saturating_add(mc6_pcc/2);
    let mut s = MODULE.lock();
    let ema = ((s.mc6_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.mc6_valid = mc6_valid; s.mc6_uncorrectable = mc6_uncorrectable; s.mc6_pcc = mc6_pcc; s.mc6_ema = ema;
    serial_println!("[msr_ia32_mc6_status] age={} lo={:#010x} hi={:#010x} valid={} unc={} pcc={} ema={}", age, lo, hi, mc6_valid, mc6_uncorrectable, mc6_pcc, ema);
}
pub fn get_mc6_valid() -> u16 { MODULE.lock().mc6_valid }
pub fn get_mc6_uncorrectable() -> u16 { MODULE.lock().mc6_uncorrectable }
pub fn get_mc6_pcc() -> u16 { MODULE.lock().mc6_pcc }
pub fn get_mc6_ema() -> u16 { MODULE.lock().mc6_ema }

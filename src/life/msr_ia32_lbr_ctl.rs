#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lbr_en: u16, lbr_call_stack: u16, lbr_filter: u16, lbr_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lbr_en: 0, lbr_call_stack: 0, lbr_filter: 0, lbr_ctl_ema: 0 });

pub fn init() { serial_println!("[msr_ia32_lbr_ctl] init"); }
pub fn tick(age: u32) {
    if age % 2500 != 0 { return; }
    let max_leaf: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max_leaf, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max_leaf < 0x1C { return; }
    let lbr_eax: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0x1Cu32 => lbr_eax, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
    if lbr_eax == 0 { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x14CEu32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let lbr_en: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let lbr_call_stack: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let filter_raw = (lo >> 2) & 0xF;
    let lbr_filter: u16 = ((filter_raw as u32 * 1000) / 15).min(1000) as u16;
    let composite: u16 = (lbr_en/4).saturating_add(lbr_call_stack/4).saturating_add(lbr_filter/2);
    let mut s = MODULE.lock();
    let ema = ((s.lbr_ctl_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.lbr_en = lbr_en; s.lbr_call_stack = lbr_call_stack; s.lbr_filter = lbr_filter; s.lbr_ctl_ema = ema;
    serial_println!("[msr_ia32_lbr_ctl] age={} lo={:#010x} en={} call_stack={} filter={} ema={}", age, lo, lbr_en, lbr_call_stack, lbr_filter, ema);
}
pub fn get_lbr_en() -> u16 { MODULE.lock().lbr_en }
pub fn get_lbr_call_stack() -> u16 { MODULE.lock().lbr_call_stack }
pub fn get_lbr_filter() -> u16 { MODULE.lock().lbr_filter }
pub fn get_lbr_ctl_ema() -> u16 { MODULE.lock().lbr_ctl_ema }

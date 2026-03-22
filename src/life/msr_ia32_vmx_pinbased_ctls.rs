#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmx_pinbased_en: u16, vmx_ext_int: u16, vmx_nmi: u16, vmx_pin_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmx_pinbased_en:0, vmx_ext_int:0, vmx_nmi:0, vmx_pin_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_pinbased_ctls] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x481u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let vmx_pinbased_en: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let vmx_ext_int: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let vmx_nmi: u16 = if (lo >> 3) & 1 != 0 { 1000 } else { 0 };
    let composite = (vmx_pinbased_en as u32/3).saturating_add(vmx_ext_int as u32/3).saturating_add(vmx_nmi as u32/3);
    let mut s = MODULE.lock();
    let vmx_pin_ema = ((s.vmx_pin_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmx_pinbased_en=vmx_pinbased_en; s.vmx_ext_int=vmx_ext_int; s.vmx_nmi=vmx_nmi; s.vmx_pin_ema=vmx_pin_ema;
    serial_println!("[msr_ia32_vmx_pinbased_ctls] age={} en={} ext_int={} nmi={} ema={}", age, vmx_pinbased_en, vmx_ext_int, vmx_nmi, vmx_pin_ema);
}
pub fn get_vmx_pinbased_en() -> u16 { MODULE.lock().vmx_pinbased_en }
pub fn get_vmx_ext_int()     -> u16 { MODULE.lock().vmx_ext_int }
pub fn get_vmx_nmi()         -> u16 { MODULE.lock().vmx_nmi }
pub fn get_vmx_pin_ema()     -> u16 { MODULE.lock().vmx_pin_ema }

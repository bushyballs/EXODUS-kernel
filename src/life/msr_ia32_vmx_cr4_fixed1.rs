#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { cr4_flex_bits: u16, cr4_smep_flex: u16, cr4_f1_density: u16, cr4_f1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { cr4_flex_bits:0, cr4_smep_flex:0, cr4_f1_density:0, cr4_f1_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_cr4_fixed1] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x489u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let bits = popcount(lo);
    let cr4_f1_density = ((bits * 62).min(1000)) as u16;
    let cr4_flex_bits = cr4_f1_density;
    let cr4_smep_flex: u16 = if (lo >> 20) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (cr4_flex_bits as u32/3).saturating_add(cr4_smep_flex as u32/3).saturating_add(cr4_f1_density as u32/3);
    let mut s = MODULE.lock();
    let cr4_f1_ema = ((s.cr4_f1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.cr4_flex_bits=cr4_flex_bits; s.cr4_smep_flex=cr4_smep_flex; s.cr4_f1_density=cr4_f1_density; s.cr4_f1_ema=cr4_f1_ema;
    serial_println!("[msr_ia32_vmx_cr4_fixed1] age={} flex={} smep={} density={} ema={}", age, cr4_flex_bits, cr4_smep_flex, cr4_f1_density, cr4_f1_ema);
}
pub fn get_cr4_flex_bits()    -> u16 { MODULE.lock().cr4_flex_bits }
pub fn get_cr4_smep_flex()    -> u16 { MODULE.lock().cr4_smep_flex }
pub fn get_cr4_f1_density()   -> u16 { MODULE.lock().cr4_f1_density }
pub fn get_cr4_f1_ema()       -> u16 { MODULE.lock().cr4_f1_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ia32e_on: u16, smx_vmx: u16, cr0_fixed_density: u16, vmx_cr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ia32e_on:0, smx_vmx:0, cr0_fixed_density:0, vmx_cr_ema:0 });

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_cr0_fixed0] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x486u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let ia32e_on: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    let smx_vmx: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let bits = popcount(lo);
    let cr0_fixed_density = ((bits * 31).min(1000)) as u16;
    let composite = (ia32e_on as u32/3).saturating_add(smx_vmx as u32/3).saturating_add(cr0_fixed_density as u32/3);
    let mut s = MODULE.lock();
    let vmx_cr_ema = ((s.vmx_cr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ia32e_on=ia32e_on; s.smx_vmx=smx_vmx; s.cr0_fixed_density=cr0_fixed_density; s.vmx_cr_ema=vmx_cr_ema;
    serial_println!("[msr_ia32_vmx_cr0_fixed0] age={} ia32e={} smx={} density={} ema={}", age, ia32e_on, smx_vmx, cr0_fixed_density, vmx_cr_ema);
}
pub fn get_ia32e_on()            -> u16 { MODULE.lock().ia32e_on }
pub fn get_smx_vmx()             -> u16 { MODULE.lock().smx_vmx }
pub fn get_cr0_fixed_density()   -> u16 { MODULE.lock().cr0_fixed_density }
pub fn get_vmx_cr_ema()          -> u16 { MODULE.lock().vmx_cr_ema }

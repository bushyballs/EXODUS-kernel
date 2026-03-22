#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { cr4_pae_required: u16, cr4_pge_required: u16, cr4_f0_density: u16, cr4_f0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { cr4_pae_required:0, cr4_pge_required:0, cr4_f0_density:0, cr4_f0_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_cr4_fixed0] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x488u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let cr4_pae_required: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    let cr4_pge_required: u16 = if (lo >> 7) & 1 != 0 { 1000 } else { 0 };
    let bits = popcount(lo);
    let cr4_f0_density = ((bits * 62).min(1000)) as u16;
    let _ = hi;
    let composite = (cr4_pae_required as u32/3).saturating_add(cr4_pge_required as u32/3).saturating_add(cr4_f0_density as u32/3);
    let mut s = MODULE.lock();
    let cr4_f0_ema = ((s.cr4_f0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.cr4_pae_required=cr4_pae_required; s.cr4_pge_required=cr4_pge_required; s.cr4_f0_density=cr4_f0_density; s.cr4_f0_ema=cr4_f0_ema;
    serial_println!("[msr_ia32_vmx_cr4_fixed0] age={} pae={} pge={} density={} ema={}", age, cr4_pae_required, cr4_pge_required, cr4_f0_density, cr4_f0_ema);
}
pub fn get_cr4_pae_required() -> u16 { MODULE.lock().cr4_pae_required }
pub fn get_cr4_pge_required() -> u16 { MODULE.lock().cr4_pge_required }
pub fn get_cr4_f0_density()   -> u16 { MODULE.lock().cr4_f0_density }
pub fn get_cr4_f0_ema()       -> u16 { MODULE.lock().cr4_f0_ema }

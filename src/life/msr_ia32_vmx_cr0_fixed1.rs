#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { cr0_flex_bits: u16, cr0_has_wp_flex: u16, cr0_fixed1_density: u16, cr0_f1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { cr0_flex_bits:0, cr0_has_wp_flex:0, cr0_fixed1_density:0, cr0_f1_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_cr0_fixed1] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x487u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let bits = popcount(lo);
    let cr0_fixed1_density = ((bits * 31).min(1000)) as u16;
    let cr0_flex_bits = cr0_fixed1_density;
    let cr0_has_wp_flex: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (cr0_flex_bits as u32/3).saturating_add(cr0_has_wp_flex as u32/3).saturating_add(cr0_fixed1_density as u32/3);
    let mut s = MODULE.lock();
    let cr0_f1_ema = ((s.cr0_f1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.cr0_flex_bits=cr0_flex_bits; s.cr0_has_wp_flex=cr0_has_wp_flex; s.cr0_fixed1_density=cr0_fixed1_density; s.cr0_f1_ema=cr0_f1_ema;
    serial_println!("[msr_ia32_vmx_cr0_fixed1] age={} flex={} wp_flex={} density={} ema={}", age, cr0_flex_bits, cr0_has_wp_flex, cr0_fixed1_density, cr0_f1_ema);
}
pub fn get_cr0_flex_bits()      -> u16 { MODULE.lock().cr0_flex_bits }
pub fn get_cr0_has_wp_flex()    -> u16 { MODULE.lock().cr0_has_wp_flex }
pub fn get_cr0_fixed1_density() -> u16 { MODULE.lock().cr0_fixed1_density }
pub fn get_cr0_f1_ema()         -> u16 { MODULE.lock().cr0_f1_ema }

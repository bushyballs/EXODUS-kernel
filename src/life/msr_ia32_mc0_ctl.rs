#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

fn popcount(mut v: u32) -> u32 { let mut c=0u32; while v!=0 { c+=v&1; v>>=1; } c }

struct State { mc0_ctl_enabled: u16, mc0_error_types: u16, mc0_ctl_density: u16, mc0_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc0_ctl_enabled:0, mc0_error_types:0, mc0_ctl_density:0, mc0_ctl_ema:0 });

#[inline]
fn has_mce() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mc0_ctl] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_mce() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x400u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let mc0_ctl_enabled: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let error_bits = popcount(lo);
    let mc0_error_types = ((error_bits * 31).min(1000)) as u16;
    let mc0_ctl_density = ((error_bits * 31).min(1000)) as u16;
    let composite = (mc0_ctl_enabled as u32/3).saturating_add(mc0_error_types as u32/3).saturating_add(mc0_ctl_density as u32/3);
    let mut s = MODULE.lock();
    let mc0_ctl_ema = ((s.mc0_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mc0_ctl_enabled=mc0_ctl_enabled; s.mc0_error_types=mc0_error_types; s.mc0_ctl_density=mc0_ctl_density; s.mc0_ctl_ema=mc0_ctl_ema;
    serial_println!("[msr_ia32_mc0_ctl] age={} en={} types={} density={} ema={}", age, mc0_ctl_enabled, mc0_error_types, mc0_ctl_density, mc0_ctl_ema);
}
pub fn get_mc0_ctl_enabled() -> u16 { MODULE.lock().mc0_ctl_enabled }
pub fn get_mc0_error_types() -> u16 { MODULE.lock().mc0_error_types }
pub fn get_mc0_ctl_density() -> u16 { MODULE.lock().mc0_ctl_density }
pub fn get_mc0_ctl_ema()     -> u16 { MODULE.lock().mc0_ctl_ema }

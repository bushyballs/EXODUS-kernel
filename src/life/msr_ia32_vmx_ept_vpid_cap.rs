#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ept_exec_only: u16, ept_2mb: u16, ept_1gb: u16, ept_cap_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ept_exec_only:0, ept_2mb:0, ept_1gb:0, ept_cap_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_ept_vpid_cap] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Cu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let ept_exec_only: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let ept_2mb: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let ept_1gb: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (ept_exec_only as u32/3).saturating_add(ept_2mb as u32/3).saturating_add(ept_1gb as u32/3);
    let mut s = MODULE.lock();
    let ept_cap_ema = ((s.ept_cap_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ept_exec_only=ept_exec_only; s.ept_2mb=ept_2mb; s.ept_1gb=ept_1gb; s.ept_cap_ema=ept_cap_ema;
    serial_println!("[msr_ia32_vmx_ept_vpid_cap] age={} xo={} 2mb={} 1gb={} ema={}", age, ept_exec_only, ept_2mb, ept_1gb, ept_cap_ema);
}
pub fn get_ept_exec_only() -> u16 { MODULE.lock().ept_exec_only }
pub fn get_ept_2mb()       -> u16 { MODULE.lock().ept_2mb }
pub fn get_ept_1gb()       -> u16 { MODULE.lock().ept_1gb }
pub fn get_ept_cap_ema()   -> u16 { MODULE.lock().ept_cap_ema }

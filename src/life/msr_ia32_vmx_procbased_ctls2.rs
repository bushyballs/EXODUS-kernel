#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ept_supported: u16, vpid_supported: u16, unrestricted_guest: u16, vmx2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ept_supported:0, vpid_supported:0, unrestricted_guest:0, vmx2_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_procbased_ctls2] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Bu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let ept_supported: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let vpid_supported: u16 = if (lo >> 5) & 1 != 0 { 1000 } else { 0 };
    let unrestricted_guest: u16 = if (lo >> 7) & 1 != 0 { 1000 } else { 0 };
    let _ = hi;
    let composite = (ept_supported as u32/3).saturating_add(vpid_supported as u32/3).saturating_add(unrestricted_guest as u32/3);
    let mut s = MODULE.lock();
    let vmx2_ema = ((s.vmx2_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ept_supported=ept_supported; s.vpid_supported=vpid_supported; s.unrestricted_guest=unrestricted_guest; s.vmx2_ema=vmx2_ema;
    serial_println!("[msr_ia32_vmx_procbased_ctls2] age={} ept={} vpid={} unrestricted={} ema={}", age, ept_supported, vpid_supported, unrestricted_guest, vmx2_ema);
}
pub fn get_ept_supported()        -> u16 { MODULE.lock().ept_supported }
pub fn get_vpid_supported()       -> u16 { MODULE.lock().vpid_supported }
pub fn get_unrestricted_guest()   -> u16 { MODULE.lock().unrestricted_guest }
pub fn get_vmx2_ema()             -> u16 { MODULE.lock().vmx2_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmxtrueentryctls_allow0: u16, vmxtrueentryctls_allow1: u16, vmxtrueentryctls_flex: u16, vmxtrueentryctls_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmxtrueentryctls_allow0:0, vmxtrueentryctls_allow1:0, vmxtrueentryctls_flex:0, vmxtrueentryctls_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack, nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_true_entry_ctls] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x490u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // lo = allowed-0 bits (must-be-0), hi = allowed-1 bits (can-be-1)
    let vmxtrueentryctls_allow0 = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let vmxtrueentryctls_allow1 = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Flexibility = bits that are 1 in allow1 but 0 in allow0 (settable either way)
    let flex_bits = hi & !lo;
    let vmxtrueentryctls_flex = ((flex_bits & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (vmxtrueentryctls_allow0 as u32/3).saturating_add(vmxtrueentryctls_allow1 as u32/3).saturating_add(vmxtrueentryctls_flex as u32/3);
    let mut s = MODULE.lock();
    let vmxtrueentryctls_ema = ((s.vmxtrueentryctls_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmxtrueentryctls_allow0=vmxtrueentryctls_allow0; s.vmxtrueentryctls_allow1=vmxtrueentryctls_allow1; s.vmxtrueentryctls_flex=vmxtrueentryctls_flex; s.vmxtrueentryctls_ema=vmxtrueentryctls_ema;
    serial_println!("[msr_ia32_vmx_true_entry_ctls] age={} a0={} a1={} flex={} ema={}", age, vmxtrueentryctls_allow0, vmxtrueentryctls_allow1, vmxtrueentryctls_flex, vmxtrueentryctls_ema);
}
pub fn get_vmxtrueentryctls_allow0() -> u16 { MODULE.lock().vmxtrueentryctls_allow0 }
pub fn get_vmxtrueentryctls_allow1() -> u16 { MODULE.lock().vmxtrueentryctls_allow1 }
pub fn get_vmxtrueentryctls_flex()   -> u16 { MODULE.lock().vmxtrueentryctls_flex }
pub fn get_vmxtrueentryctls_ema()    -> u16 { MODULE.lock().vmxtrueentryctls_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmxtrueexitctls_allow0: u16, vmxtrueexitctls_allow1: u16, vmxtrueexitctls_flex: u16, vmxtrueexitctls_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmxtrueexitctls_allow0:0, vmxtrueexitctls_allow1:0, vmxtrueexitctls_flex:0, vmxtrueexitctls_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack, nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_true_exit_ctls] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Fu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // lo = allowed-0 bits (must-be-0), hi = allowed-1 bits (can-be-1)
    let vmxtrueexitctls_allow0 = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let vmxtrueexitctls_allow1 = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Flexibility = bits that are 1 in allow1 but 0 in allow0 (settable either way)
    let flex_bits = hi & !lo;
    let vmxtrueexitctls_flex = ((flex_bits & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (vmxtrueexitctls_allow0 as u32/3).saturating_add(vmxtrueexitctls_allow1 as u32/3).saturating_add(vmxtrueexitctls_flex as u32/3);
    let mut s = MODULE.lock();
    let vmxtrueexitctls_ema = ((s.vmxtrueexitctls_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmxtrueexitctls_allow0=vmxtrueexitctls_allow0; s.vmxtrueexitctls_allow1=vmxtrueexitctls_allow1; s.vmxtrueexitctls_flex=vmxtrueexitctls_flex; s.vmxtrueexitctls_ema=vmxtrueexitctls_ema;
    serial_println!("[msr_ia32_vmx_true_exit_ctls] age={} a0={} a1={} flex={} ema={}", age, vmxtrueexitctls_allow0, vmxtrueexitctls_allow1, vmxtrueexitctls_flex, vmxtrueexitctls_ema);
}
pub fn get_vmxtrueexitctls_allow0() -> u16 { MODULE.lock().vmxtrueexitctls_allow0 }
pub fn get_vmxtrueexitctls_allow1() -> u16 { MODULE.lock().vmxtrueexitctls_allow1 }
pub fn get_vmxtrueexitctls_flex()   -> u16 { MODULE.lock().vmxtrueexitctls_flex }
pub fn get_vmxtrueexitctls_ema()    -> u16 { MODULE.lock().vmxtrueexitctls_ema }

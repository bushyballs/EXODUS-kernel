#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmxtrueprocbasedctls_allow0: u16, vmxtrueprocbasedctls_allow1: u16, vmxtrueprocbasedctls_flex: u16, vmxtrueprocbasedctls_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmxtrueprocbasedctls_allow0:0, vmxtrueprocbasedctls_allow1:0, vmxtrueprocbasedctls_flex:0, vmxtrueprocbasedctls_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack, nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_true_procbased_ctls] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Eu32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // lo = allowed-0 bits (must-be-0), hi = allowed-1 bits (can-be-1)
    let vmxtrueprocbasedctls_allow0 = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let vmxtrueprocbasedctls_allow1 = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Flexibility = bits that are 1 in allow1 but 0 in allow0 (settable either way)
    let flex_bits = hi & !lo;
    let vmxtrueprocbasedctls_flex = ((flex_bits & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (vmxtrueprocbasedctls_allow0 as u32/3).saturating_add(vmxtrueprocbasedctls_allow1 as u32/3).saturating_add(vmxtrueprocbasedctls_flex as u32/3);
    let mut s = MODULE.lock();
    let vmxtrueprocbasedctls_ema = ((s.vmxtrueprocbasedctls_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmxtrueprocbasedctls_allow0=vmxtrueprocbasedctls_allow0; s.vmxtrueprocbasedctls_allow1=vmxtrueprocbasedctls_allow1; s.vmxtrueprocbasedctls_flex=vmxtrueprocbasedctls_flex; s.vmxtrueprocbasedctls_ema=vmxtrueprocbasedctls_ema;
    serial_println!("[msr_ia32_vmx_true_procbased_ctls] age={} a0={} a1={} flex={} ema={}", age, vmxtrueprocbasedctls_allow0, vmxtrueprocbasedctls_allow1, vmxtrueprocbasedctls_flex, vmxtrueprocbasedctls_ema);
}
pub fn get_vmxtrueprocbasedctls_allow0() -> u16 { MODULE.lock().vmxtrueprocbasedctls_allow0 }
pub fn get_vmxtrueprocbasedctls_allow1() -> u16 { MODULE.lock().vmxtrueprocbasedctls_allow1 }
pub fn get_vmxtrueprocbasedctls_flex()   -> u16 { MODULE.lock().vmxtrueprocbasedctls_flex }
pub fn get_vmxtrueprocbasedctls_ema()    -> u16 { MODULE.lock().vmxtrueprocbasedctls_ema }

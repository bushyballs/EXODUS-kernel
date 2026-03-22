#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmxtruepinbasedctls_allow0: u16, vmxtruepinbasedctls_allow1: u16, vmxtruepinbasedctls_flex: u16, vmxtruepinbasedctls_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmxtruepinbasedctls_allow0:0, vmxtruepinbasedctls_allow1:0, vmxtruepinbasedctls_flex:0, vmxtruepinbasedctls_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack, nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_true_pinbased_ctls] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x48Du32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // lo = allowed-0 bits (must-be-0), hi = allowed-1 bits (can-be-1)
    let vmxtruepinbasedctls_allow0 = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let vmxtruepinbasedctls_allow1 = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    // Flexibility = bits that are 1 in allow1 but 0 in allow0 (settable either way)
    let flex_bits = hi & !lo;
    let vmxtruepinbasedctls_flex = ((flex_bits & 0xFFFF) * 1000 / 65535) as u16;
    let composite = (vmxtruepinbasedctls_allow0 as u32/3).saturating_add(vmxtruepinbasedctls_allow1 as u32/3).saturating_add(vmxtruepinbasedctls_flex as u32/3);
    let mut s = MODULE.lock();
    let vmxtruepinbasedctls_ema = ((s.vmxtruepinbasedctls_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmxtruepinbasedctls_allow0=vmxtruepinbasedctls_allow0; s.vmxtruepinbasedctls_allow1=vmxtruepinbasedctls_allow1; s.vmxtruepinbasedctls_flex=vmxtruepinbasedctls_flex; s.vmxtruepinbasedctls_ema=vmxtruepinbasedctls_ema;
    serial_println!("[msr_ia32_vmx_true_pinbased_ctls] age={} a0={} a1={} flex={} ema={}", age, vmxtruepinbasedctls_allow0, vmxtruepinbasedctls_allow1, vmxtruepinbasedctls_flex, vmxtruepinbasedctls_ema);
}
pub fn get_vmxtruepinbasedctls_allow0() -> u16 { MODULE.lock().vmxtruepinbasedctls_allow0 }
pub fn get_vmxtruepinbasedctls_allow1() -> u16 { MODULE.lock().vmxtruepinbasedctls_allow1 }
pub fn get_vmxtruepinbasedctls_flex()   -> u16 { MODULE.lock().vmxtruepinbasedctls_flex }
pub fn get_vmxtruepinbasedctls_ema()    -> u16 { MODULE.lock().vmxtruepinbasedctls_ema }

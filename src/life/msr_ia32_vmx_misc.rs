#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { vmx_misc_cr3: u16, vmx_misc_smm: u16, vmx_misc_flex: u16, vmx_misc_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { vmx_misc_cr3:0, vmx_misc_smm:0, vmx_misc_flex:0, vmx_misc_ema:0 });

#[inline]
fn has_vmx() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack, nomem)); }
    (ecx >> 5) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_vmx_misc] init"); }
pub fn tick(age: u32) {
    if age % 7000 != 0 { return; }
    if !has_vmx() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x485u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[4:0]: VMX preemption timer rate
    let timer_rate = lo & 0x1F;
    let vmx_misc_cr3 = ((timer_rate * 1000) / 31) as u16;
    // bit 14: SMM inside VMX operation supported
    let vmx_misc_smm: u16 = if (lo >> 14) & 1 != 0 { 1000 } else { 0 };
    // bits[24:16]: number of CR3 targets supported (max flexibility)
    let cr3_targets = (lo >> 16) & 0xFF;
    let vmx_misc_flex = ((cr3_targets * 1000) / 255) as u16;
    let composite = (vmx_misc_cr3 as u32/3).saturating_add(vmx_misc_smm as u32/3).saturating_add(vmx_misc_flex as u32/3);
    let mut s = MODULE.lock();
    let vmx_misc_ema = ((s.vmx_misc_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.vmx_misc_cr3=vmx_misc_cr3; s.vmx_misc_smm=vmx_misc_smm; s.vmx_misc_flex=vmx_misc_flex; s.vmx_misc_ema=vmx_misc_ema;
    serial_println!("[msr_ia32_vmx_misc] age={} cr3={} smm={} flex={} ema={}", age, vmx_misc_cr3, vmx_misc_smm, vmx_misc_flex, vmx_misc_ema);
}
pub fn get_vmx_misc_cr3()  -> u16 { MODULE.lock().vmx_misc_cr3 }
pub fn get_vmx_misc_smm()  -> u16 { MODULE.lock().vmx_misc_smm }
pub fn get_vmx_misc_flex() -> u16 { MODULE.lock().vmx_misc_flex }
pub fn get_vmx_misc_ema()  -> u16 { MODULE.lock().vmx_misc_ema }

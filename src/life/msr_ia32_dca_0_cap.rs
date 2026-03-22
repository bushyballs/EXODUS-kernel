#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { dca_type0_active: u16, dca_run_enable: u16, dca_hw_disable: u16, dca_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { dca_type0_active: 0, dca_run_enable: 0, dca_hw_disable: 0, dca_ema: 0 });

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 15) & 1 == 1
}
fn has_dca() -> bool {
    let max: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 0u32 => max, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    if max < 9 { return false; }
    let dca_eax: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 9u32 => dca_eax, inout("ecx") 0u32 => _, lateout("edx") _, options(nostack,nomem)); }
    dca_eax != 0
}
pub fn init() { serial_println!("[msr_ia32_dca_0_cap] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_pdcm() || !has_dca() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x1FBu32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let dca_type0_active: u16 = if lo & 1 != 0 { 1000 } else { 0 };
    let delay = (lo >> 4) & 0xF;
    let dca_run_enable: u16 = ((delay as u32 * 1000) / 15).min(1000) as u16;
    let dca_hw_disable: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    let composite: u16 = (dca_type0_active/4).saturating_add(dca_run_enable/4).saturating_add(dca_hw_disable/2);
    let mut s = MODULE.lock();
    let ema = ((s.dca_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.dca_type0_active = dca_type0_active; s.dca_run_enable = dca_run_enable; s.dca_hw_disable = dca_hw_disable; s.dca_ema = ema;
    serial_println!("[msr_ia32_dca_0_cap] age={} lo={:#010x} type0={} run_en={} hw_dis={} ema={}", age, lo, dca_type0_active, dca_run_enable, dca_hw_disable, ema);
}
pub fn get_dca_type0_active() -> u16 { MODULE.lock().dca_type0_active }
pub fn get_dca_run_enable() -> u16 { MODULE.lock().dca_run_enable }
pub fn get_dca_hw_disable() -> u16 { MODULE.lock().dca_hw_disable }
pub fn get_dca_ema() -> u16 { MODULE.lock().dca_ema }

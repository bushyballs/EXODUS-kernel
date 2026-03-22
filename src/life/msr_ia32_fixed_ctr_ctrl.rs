#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ctr0_active: u16, ctr1_active: u16, ctr2_active: u16, fixed_ctrl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ctr0_active: 0, ctr1_active: 0, ctr2_active: 0, fixed_ctrl_ema: 0 });

fn has_pdcm() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 15) & 1 == 1
}
pub fn init() { serial_println!("[msr_ia32_fixed_ctr_ctrl] init"); }
pub fn tick(age: u32) {
    if age % 1500 != 0 { return; }
    if !has_pdcm() { return; }
    let lo: u32; let _hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x38Du32, out("eax") lo, out("edx") _hi, options(nostack,nomem)); }
    let ctr0_active: u16 = if lo & 0x3 != 0 { 1000 } else { 0 };
    let ctr1_active: u16 = if (lo >> 4) & 0x3 != 0 { 1000 } else { 0 };
    let ctr2_active: u16 = if (lo >> 8) & 0x3 != 0 { 1000 } else { 0 };
    let composite: u16 = (ctr0_active/4).saturating_add(ctr1_active/4).saturating_add(ctr2_active/2);
    let mut s = MODULE.lock();
    let ema = ((s.fixed_ctrl_ema as u32).wrapping_mul(7).saturating_add(composite as u32)/8).min(1000) as u16;
    s.ctr0_active = ctr0_active; s.ctr1_active = ctr1_active; s.ctr2_active = ctr2_active; s.fixed_ctrl_ema = ema;
    serial_println!("[msr_ia32_fixed_ctr_ctrl] age={} lo={:#010x} ctr0={} ctr1={} ctr2={} ema={}", age, lo, ctr0_active, ctr1_active, ctr2_active, ema);
}
pub fn get_ctr0_active() -> u16 { MODULE.lock().ctr0_active }
pub fn get_ctr1_active() -> u16 { MODULE.lock().ctr1_active }
pub fn get_ctr2_active() -> u16 { MODULE.lock().ctr2_active }
pub fn get_fixed_ctrl_ema() -> u16 { MODULE.lock().fixed_ctrl_ema }

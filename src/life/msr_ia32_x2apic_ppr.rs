#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ppr_level: u16, ppr_active: u16, ppr_class: u16, ppr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ppr_level:0, ppr_active:0, ppr_class:0, ppr_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_ppr] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x80Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let raw = lo & 0xFF;
    let ppr_level = ((raw * 1000) / 255).min(1000) as u16;
    let ppr_active: u16 = if raw != 0 { 1000 } else { 0 };
    let ppr_class = ((raw >> 4) * 111).min(1000) as u16;
    let composite = (ppr_level as u32/3).saturating_add(ppr_active as u32/3).saturating_add(ppr_class as u32/3);
    let mut s = MODULE.lock();
    let ppr_ema = ((s.ppr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ppr_level=ppr_level; s.ppr_active=ppr_active; s.ppr_class=ppr_class; s.ppr_ema=ppr_ema;
    serial_println!("[msr_ia32_x2apic_ppr] age={} level={} active={} class={} ema={}", age, ppr_level, ppr_active, ppr_class, ppr_ema);
}
pub fn get_ppr_level()  -> u16 { MODULE.lock().ppr_level }
pub fn get_ppr_active() -> u16 { MODULE.lock().ppr_active }
pub fn get_ppr_class()  -> u16 { MODULE.lock().ppr_class }
pub fn get_ppr_ema()    -> u16 { MODULE.lock().ppr_ema }

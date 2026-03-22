#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { tpr_level: u16, tpr_active: u16, tpr_class: u16, tpr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { tpr_level:0, tpr_active:0, tpr_class:0, tpr_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_tpr] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x808u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let raw = lo & 0xFF;
    let tpr_level = ((raw * 1000) / 255).min(1000) as u16;
    let tpr_active: u16 = if raw != 0 { 1000 } else { 0 };
    let tpr_class = ((raw >> 4) * 111).min(1000) as u16;
    let composite = (tpr_level as u32/3).saturating_add(tpr_active as u32/3).saturating_add(tpr_class as u32/3);
    let mut s = MODULE.lock();
    let tpr_ema = ((s.tpr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.tpr_level=tpr_level; s.tpr_active=tpr_active; s.tpr_class=tpr_class; s.tpr_ema=tpr_ema;
    serial_println!("[msr_ia32_x2apic_tpr] age={} level={} active={} class={} ema={}", age, tpr_level, tpr_active, tpr_class, tpr_ema);
}
pub fn get_tpr_level()  -> u16 { MODULE.lock().tpr_level }
pub fn get_tpr_active() -> u16 { MODULE.lock().tpr_active }
pub fn get_tpr_class()  -> u16 { MODULE.lock().tpr_class }
pub fn get_tpr_ema()    -> u16 { MODULE.lock().tpr_ema }

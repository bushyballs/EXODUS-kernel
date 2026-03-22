#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { self_ipi_vec: u16, self_ipi_active: u16, self_ipi_class: u16, ipi_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { self_ipi_vec:0, self_ipi_active:0, self_ipi_class:0, ipi_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_self_ipi] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x83Fu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let vec_raw = lo & 0xFF;
    let self_ipi_vec = ((vec_raw * 1000) / 255).min(1000) as u16;
    let self_ipi_active: u16 = if vec_raw >= 16 { 1000 } else { 0 };
    let self_ipi_class = ((vec_raw >> 4) * 62).min(1000) as u16;
    let composite = (self_ipi_vec as u32/3).saturating_add(self_ipi_active as u32/3).saturating_add(self_ipi_class as u32/3);
    let mut s = MODULE.lock();
    let ipi_ema = ((s.ipi_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.self_ipi_vec=self_ipi_vec; s.self_ipi_active=self_ipi_active; s.self_ipi_class=self_ipi_class; s.ipi_ema=ipi_ema;
    serial_println!("[msr_ia32_x2apic_self_ipi] age={} vec={} active={} class={} ema={}", age, self_ipi_vec, self_ipi_active, self_ipi_class, ipi_ema);
}
pub fn get_self_ipi_vec()    -> u16 { MODULE.lock().self_ipi_vec }
pub fn get_self_ipi_active() -> u16 { MODULE.lock().self_ipi_active }
pub fn get_self_ipi_class()  -> u16 { MODULE.lock().self_ipi_class }
pub fn get_ipi_ema()         -> u16 { MODULE.lock().ipi_ema }

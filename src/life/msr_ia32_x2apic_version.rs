#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { x2apic_ver: u16, max_lvt: u16, directed_eoi: u16, apic_ver_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { x2apic_ver:0, max_lvt:0, directed_eoi:0, apic_ver_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_version] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x803u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let ver_raw = lo & 0xFF;
    let x2apic_ver = ((ver_raw * 1000) / 255).min(1000) as u16;
    let lvt_raw = (lo >> 16) & 0xFF;
    let max_lvt = ((lvt_raw * 1000) / 7).min(1000) as u16;
    let directed_eoi: u16 = if (lo >> 24) & 1 != 0 { 1000 } else { 0 };
    let composite = (x2apic_ver as u32/3).saturating_add(max_lvt as u32/3).saturating_add(directed_eoi as u32/3);
    let mut s = MODULE.lock();
    let apic_ver_ema = ((s.apic_ver_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.x2apic_ver=x2apic_ver; s.max_lvt=max_lvt; s.directed_eoi=directed_eoi; s.apic_ver_ema=apic_ver_ema;
    serial_println!("[msr_ia32_x2apic_version] age={} ver={} lvt={} deoi={} ema={}", age, x2apic_ver, max_lvt, directed_eoi, apic_ver_ema);
}
pub fn get_x2apic_ver()    -> u16 { MODULE.lock().x2apic_ver }
pub fn get_max_lvt()       -> u16 { MODULE.lock().max_lvt }
pub fn get_directed_eoi()  -> u16 { MODULE.lock().directed_eoi }
pub fn get_apic_ver_ema()  -> u16 { MODULE.lock().apic_ver_ema }

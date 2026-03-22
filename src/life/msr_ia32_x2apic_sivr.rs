#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { spurious_vector: u16, apic_sw_en: u16, focus_checking: u16, sivr_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { spurious_vector:0, apic_sw_en:0, focus_checking:0, sivr_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_sivr] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x8F0u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let vec_raw = lo & 0xFF;
    let spurious_vector = ((vec_raw * 1000) / 255).min(1000) as u16;
    let apic_sw_en: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    let focus_checking: u16 = if (lo >> 9) & 1 != 0 { 0 } else { 1000 };
    let composite = (spurious_vector as u32/4).saturating_add(apic_sw_en as u32/2).saturating_add(focus_checking as u32/4);
    let mut s = MODULE.lock();
    let sivr_ema = ((s.sivr_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.spurious_vector=spurious_vector; s.apic_sw_en=apic_sw_en; s.focus_checking=focus_checking; s.sivr_ema=sivr_ema;
    serial_println!("[msr_ia32_x2apic_sivr] age={} vec={} sw_en={} focus={} ema={}", age, spurious_vector, apic_sw_en, focus_checking, sivr_ema);
}
pub fn get_spurious_vector() -> u16 { MODULE.lock().spurious_vector }
pub fn get_apic_sw_en()      -> u16 { MODULE.lock().apic_sw_en }
pub fn get_focus_checking()  -> u16 { MODULE.lock().focus_checking }
pub fn get_sivr_ema()        -> u16 { MODULE.lock().sivr_ema }

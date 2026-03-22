#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ipi_pending: u16, ipi_delivery_mode: u16, ipi_dest_shorthand: u16, ipi_activity_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ipi_pending:0, ipi_delivery_mode:0, ipi_dest_shorthand:0, ipi_activity_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_icr] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x830u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let ipi_pending: u16 = if (lo >> 12) & 1 != 0 { 1000 } else { 0 };
    let del_mode = (lo >> 8) & 0x7;
    let ipi_delivery_mode = ((del_mode * 142).min(1000)) as u16;
    let shorthand = (lo >> 18) & 0x3;
    let ipi_dest_shorthand = ((shorthand * 333).min(1000)) as u16;
    let _ = hi;
    let composite = (ipi_pending as u32/3).saturating_add(ipi_delivery_mode as u32/3).saturating_add(ipi_dest_shorthand as u32/3);
    let mut s = MODULE.lock();
    let ipi_activity_ema = ((s.ipi_activity_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ipi_pending=ipi_pending; s.ipi_delivery_mode=ipi_delivery_mode; s.ipi_dest_shorthand=ipi_dest_shorthand; s.ipi_activity_ema=ipi_activity_ema;
    serial_println!("[msr_ia32_x2apic_icr] age={} pending={} del={} short={} ema={}", age, ipi_pending, ipi_delivery_mode, ipi_dest_shorthand, ipi_activity_ema);
}
pub fn get_ipi_pending()        -> u16 { MODULE.lock().ipi_pending }
pub fn get_ipi_delivery_mode()  -> u16 { MODULE.lock().ipi_delivery_mode }
pub fn get_ipi_dest_shorthand() -> u16 { MODULE.lock().ipi_dest_shorthand }
pub fn get_ipi_activity_ema()   -> u16 { MODULE.lock().ipi_activity_ema }

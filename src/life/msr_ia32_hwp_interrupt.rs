#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { perf_change_irq: u16, energy_change_irq: u16, hwp_irq_active: u16, hwp_irq_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { perf_change_irq:0, energy_change_irq:0, hwp_irq_active:0, hwp_irq_ema:0 });

#[inline]
fn has_hwp() -> bool {
    let eax: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 6u32 => eax, lateout("ecx") _, lateout("edx") _, options(nostack,nomem)); }
    (eax >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_hwp_interrupt] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_hwp() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x773u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let perf_change_irq: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let energy_change_irq: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let hwp_irq_active: u16 = if lo & 0x3 != 0 { 1000 } else { 0 };
    let composite = (perf_change_irq as u32/3).saturating_add(energy_change_irq as u32/3).saturating_add(hwp_irq_active as u32/3);
    let mut s = MODULE.lock();
    let hwp_irq_ema = ((s.hwp_irq_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.perf_change_irq=perf_change_irq; s.energy_change_irq=energy_change_irq; s.hwp_irq_active=hwp_irq_active; s.hwp_irq_ema=hwp_irq_ema;
    serial_println!("[msr_ia32_hwp_interrupt] age={} perf_irq={} energy_irq={} active={} ema={}", age, perf_change_irq, energy_change_irq, hwp_irq_active, hwp_irq_ema);
}
pub fn get_perf_change_irq()   -> u16 { MODULE.lock().perf_change_irq }
pub fn get_energy_change_irq() -> u16 { MODULE.lock().energy_change_irq }
pub fn get_hwp_irq_active()    -> u16 { MODULE.lock().hwp_irq_active }
pub fn get_hwp_irq_ema()       -> u16 { MODULE.lock().hwp_irq_ema }

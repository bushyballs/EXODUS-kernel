#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { timer_vector: u16, timer_mode: u16, timer_masked: u16, lvt_timer_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { timer_vector:0, timer_mode:0, timer_masked:0, lvt_timer_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_lvt_timer] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x832u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let vec_raw = lo & 0xFF;
    let timer_vector = ((vec_raw * 1000) / 255).min(1000) as u16;
    let mode_raw = (lo >> 17) & 0x3;
    let timer_mode = ((mode_raw * 333).min(1000)) as u16;
    let timer_masked: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let active = 1000u16.saturating_sub(timer_masked);
    let composite = (timer_vector as u32/4).saturating_add(timer_mode as u32/4).saturating_add(active as u32/2);
    let mut s = MODULE.lock();
    let lvt_timer_ema = ((s.lvt_timer_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.timer_vector=timer_vector; s.timer_mode=timer_mode; s.timer_masked=timer_masked; s.lvt_timer_ema=lvt_timer_ema;
    serial_println!("[msr_ia32_x2apic_lvt_timer] age={} vec={} mode={} masked={} ema={}", age, timer_vector, timer_mode, timer_masked, lvt_timer_ema);
}
pub fn get_timer_vector()   -> u16 { MODULE.lock().timer_vector }
pub fn get_timer_mode()     -> u16 { MODULE.lock().timer_mode }
pub fn get_timer_masked()   -> u16 { MODULE.lock().timer_masked }
pub fn get_lvt_timer_ema()  -> u16 { MODULE.lock().lvt_timer_ema }

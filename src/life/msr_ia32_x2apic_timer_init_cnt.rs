#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { timer_init: u16, timer_armed: u16, timer_magnitude: u16, timer_init_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { timer_init:0, timer_armed:0, timer_magnitude:0, timer_init_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_timer_init_cnt] init"); }
pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x838u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let timer_armed: u16 = if lo != 0 { 1000 } else { 0 };
    let timer_magnitude = (lo >> 22).min(1000) as u16;
    let timer_init = timer_armed;
    let composite = (timer_init as u32/3).saturating_add(timer_armed as u32/3).saturating_add(timer_magnitude as u32/3);
    let mut s = MODULE.lock();
    let timer_init_ema = ((s.timer_init_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.timer_init=timer_init; s.timer_armed=timer_armed; s.timer_magnitude=timer_magnitude; s.timer_init_ema=timer_init_ema;
    serial_println!("[msr_ia32_x2apic_timer_init_cnt] age={} init={} armed={} mag={} ema={}", age, timer_init, timer_armed, timer_magnitude, timer_init_ema);
}
pub fn get_timer_init()      -> u16 { MODULE.lock().timer_init }
pub fn get_timer_armed()     -> u16 { MODULE.lock().timer_armed }
pub fn get_timer_magnitude() -> u16 { MODULE.lock().timer_magnitude }
pub fn get_timer_init_ema()  -> u16 { MODULE.lock().timer_init_ema }

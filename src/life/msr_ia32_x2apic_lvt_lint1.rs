#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lint1_vector: u16, lint1_masked: u16, lint1_mode: u16, lint1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lint1_vector:0, lint1_masked:0, lint1_mode:0, lint1_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_lvt_lint1] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x836u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let vec_raw = lo & 0xFF;
    let lint1_vector = ((vec_raw * 1000) / 255).min(1000) as u16;
    let lint1_masked: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let mode_raw = (lo >> 8) & 0x7;
    let lint1_mode = ((mode_raw * 142).min(1000)) as u16;
    let active = 1000u16.saturating_sub(lint1_masked);
    let composite = (lint1_vector as u32/4).saturating_add(active as u32/2).saturating_add(lint1_mode as u32/4);
    let mut s = MODULE.lock();
    let lint1_ema = ((s.lint1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lint1_vector=lint1_vector; s.lint1_masked=lint1_masked; s.lint1_mode=lint1_mode; s.lint1_ema=lint1_ema;
    serial_println!("[msr_ia32_x2apic_lvt_lint1] age={} vec={} masked={} mode={} ema={}", age, lint1_vector, lint1_masked, lint1_mode, lint1_ema);
}
pub fn get_lint1_vector() -> u16 { MODULE.lock().lint1_vector }
pub fn get_lint1_masked() -> u16 { MODULE.lock().lint1_masked }
pub fn get_lint1_mode()   -> u16 { MODULE.lock().lint1_mode }
pub fn get_lint1_ema()    -> u16 { MODULE.lock().lint1_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { lint0_vector: u16, lint0_masked: u16, lint0_mode: u16, lint0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { lint0_vector:0, lint0_masked:0, lint0_mode:0, lint0_ema:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_lvt_lint0] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x835u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let vec_raw = lo & 0xFF;
    let lint0_vector = ((vec_raw * 1000) / 255).min(1000) as u16;
    let lint0_masked: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let mode_raw = (lo >> 8) & 0x7;
    let lint0_mode = ((mode_raw * 142).min(1000)) as u16;
    let active = 1000u16.saturating_sub(lint0_masked);
    let composite = (lint0_vector as u32/4).saturating_add(active as u32/2).saturating_add(lint0_mode as u32/4);
    let mut s = MODULE.lock();
    let lint0_ema = ((s.lint0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.lint0_vector=lint0_vector; s.lint0_masked=lint0_masked; s.lint0_mode=lint0_mode; s.lint0_ema=lint0_ema;
    serial_println!("[msr_ia32_x2apic_lvt_lint0] age={} vec={} masked={} mode={} ema={}", age, lint0_vector, lint0_masked, lint0_mode, lint0_ema);
}
pub fn get_lint0_vector() -> u16 { MODULE.lock().lint0_vector }
pub fn get_lint0_masked() -> u16 { MODULE.lock().lint0_masked }
pub fn get_lint0_mode()   -> u16 { MODULE.lock().lint0_mode }
pub fn get_lint0_ema()    -> u16 { MODULE.lock().lint0_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { ds_area_lo: u16, ds_area_hi: u16, ds_configured: u16, ds_area_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { ds_area_lo:0, ds_area_hi:0, ds_configured:0, ds_area_ema:0 });

pub fn init() { serial_println!("[msr_ia32_ds_area] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // Check DS/PEBS support (CPUID 1 EDX bit 21)
    let edx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 21) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x600u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let ds_area_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let ds_area_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let ds_configured: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (ds_area_lo as u32/3).saturating_add(ds_area_hi as u32/3).saturating_add(ds_configured as u32/3);
    let mut s = MODULE.lock();
    let ds_area_ema = ((s.ds_area_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.ds_area_lo=ds_area_lo; s.ds_area_hi=ds_area_hi; s.ds_configured=ds_configured; s.ds_area_ema=ds_area_ema;
    serial_println!("[msr_ia32_ds_area] age={} lo={} hi={} cfg={} ema={}", age, ds_area_lo, ds_area_hi, ds_configured, ds_area_ema);
}
pub fn get_ds_area_lo()    -> u16 { MODULE.lock().ds_area_lo }
pub fn get_ds_area_hi()    -> u16 { MODULE.lock().ds_area_hi }
pub fn get_ds_configured() -> u16 { MODULE.lock().ds_configured }
pub fn get_ds_area_ema()   -> u16 { MODULE.lock().ds_area_ema }

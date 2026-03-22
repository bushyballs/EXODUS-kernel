#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pebs_frontend_idc: u16, pebs_frontend_dsb: u16, pebs_frontend_en: u16, pebs_frontend_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pebs_frontend_idc:0, pebs_frontend_dsb:0, pebs_frontend_en:0, pebs_frontend_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pebs_frontend] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let edx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 21) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3F7u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: IDQ_BUBBLE_UP enable
    let pebs_frontend_idc: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: DSB_MISS enable
    let pebs_frontend_dsb: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let pebs_frontend_en: u16 = if lo != 0 { 1000 } else { 0 };
    let composite = (pebs_frontend_idc as u32/3).saturating_add(pebs_frontend_dsb as u32/3).saturating_add(pebs_frontend_en as u32/3);
    let mut s = MODULE.lock();
    let pebs_frontend_ema = ((s.pebs_frontend_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pebs_frontend_idc=pebs_frontend_idc; s.pebs_frontend_dsb=pebs_frontend_dsb; s.pebs_frontend_en=pebs_frontend_en; s.pebs_frontend_ema=pebs_frontend_ema;
    serial_println!("[msr_ia32_pebs_frontend] age={} idc={} dsb={} en={} ema={}", age, pebs_frontend_idc, pebs_frontend_dsb, pebs_frontend_en, pebs_frontend_ema);
}
pub fn get_pebs_frontend_idc() -> u16 { MODULE.lock().pebs_frontend_idc }
pub fn get_pebs_frontend_dsb() -> u16 { MODULE.lock().pebs_frontend_dsb }
pub fn get_pebs_frontend_en()  -> u16 { MODULE.lock().pebs_frontend_en }
pub fn get_pebs_frontend_ema() -> u16 { MODULE.lock().pebs_frontend_ema }

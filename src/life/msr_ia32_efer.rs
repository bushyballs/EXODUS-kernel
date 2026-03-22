#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { efer_sce: u16, efer_lme: u16, efer_nxe: u16, efer_health_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { efer_sce:0, efer_lme:0, efer_nxe:0, efer_health_ema:0 });

pub fn init() { serial_println!("[msr_ia32_efer] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000080u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let efer_sce: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let efer_lme: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    let efer_nxe: u16 = if (lo >> 11) & 1 != 0 { 1000 } else { 0 };
    let composite = (efer_sce as u32/3).saturating_add(efer_lme as u32/3).saturating_add(efer_nxe as u32/3);
    let mut s = MODULE.lock();
    let efer_health_ema = ((s.efer_health_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.efer_sce=efer_sce; s.efer_lme=efer_lme; s.efer_nxe=efer_nxe; s.efer_health_ema=efer_health_ema;
    serial_println!("[msr_ia32_efer] age={} sce={} lme={} nxe={} ema={}", age, efer_sce, efer_lme, efer_nxe, efer_health_ema);
}
pub fn get_efer_sce()        -> u16 { MODULE.lock().efer_sce }
pub fn get_efer_lme()        -> u16 { MODULE.lock().efer_lme }
pub fn get_efer_nxe()        -> u16 { MODULE.lock().efer_nxe }
pub fn get_efer_health_ema() -> u16 { MODULE.lock().efer_health_ema }

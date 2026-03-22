#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mcg_ripv: u16, mcg_eipv: u16, mcg_mcip: u16, mcg_status_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mcg_ripv:0, mcg_eipv:0, mcg_mcip:0, mcg_status_ema:0 });

#[inline]
fn has_mce() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mcg_status] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_mce() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x17Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let mcg_ripv: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let mcg_eipv: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let mcg_mcip: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let health = 1000u32.saturating_sub(mcg_mcip as u32);
    let composite = (mcg_ripv as u32/4).saturating_add(mcg_eipv as u32/4).saturating_add(health/2);
    let mut s = MODULE.lock();
    let mcg_status_ema = ((s.mcg_status_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mcg_ripv=mcg_ripv; s.mcg_eipv=mcg_eipv; s.mcg_mcip=mcg_mcip; s.mcg_status_ema=mcg_status_ema;
    serial_println!("[msr_ia32_mcg_status] age={} ripv={} eipv={} mcip={} ema={}", age, mcg_ripv, mcg_eipv, mcg_mcip, mcg_status_ema);
}
pub fn get_mcg_ripv()       -> u16 { MODULE.lock().mcg_ripv }
pub fn get_mcg_eipv()       -> u16 { MODULE.lock().mcg_eipv }
pub fn get_mcg_mcip()       -> u16 { MODULE.lock().mcg_mcip }
pub fn get_mcg_status_ema() -> u16 { MODULE.lock().mcg_status_ema }

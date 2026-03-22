#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pm2_cntrs_lo: u16, pm2_cntrs_hi: u16, pm2_active: u16, pm2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pm2_cntrs_lo:0, pm2_cntrs_hi:0, pm2_active:0, pm2_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pm_enable2] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    // IA32_PM_CTL1 (0xDB1): PM control extension register
    let lo: u32;
    let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xDB1u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let pm2_cntrs_lo = ((lo & 0xFFFF) * 1000 / 65535) as u16;
    let pm2_cntrs_hi = ((hi & 0xFFFF) * 1000 / 65535) as u16;
    let pm2_active: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (pm2_cntrs_lo as u32/3).saturating_add(pm2_cntrs_hi as u32/3).saturating_add(pm2_active as u32/3);
    let mut s = MODULE.lock();
    let pm2_ema = ((s.pm2_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pm2_cntrs_lo=pm2_cntrs_lo; s.pm2_cntrs_hi=pm2_cntrs_hi; s.pm2_active=pm2_active; s.pm2_ema=pm2_ema;
    serial_println!("[msr_ia32_pm_enable2] age={} lo={} hi={} active={} ema={}", age, pm2_cntrs_lo, pm2_cntrs_hi, pm2_active, pm2_ema);
}
pub fn get_pm2_cntrs_lo() -> u16 { MODULE.lock().pm2_cntrs_lo }
pub fn get_pm2_cntrs_hi() -> u16 { MODULE.lock().pm2_cntrs_hi }
pub fn get_pm2_active()   -> u16 { MODULE.lock().pm2_active }
pub fn get_pm2_ema()      -> u16 { MODULE.lock().pm2_ema }

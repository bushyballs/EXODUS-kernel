#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc3_ctl_en: u16, mc3_err_valid: u16, mc3_uc_err: u16, mc3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc3_ctl_en:0, mc3_err_valid:0, mc3_uc_err:0, mc3_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mc3_ctl_status] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let ctl: u32; let sts: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x40Cu32, out("eax") ctl, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x40Du32, out("eax") sts, out("edx") _, options(nostack, nomem));
    }
    let mc3_ctl_en = ((ctl & 0xFF) * 1000 / 255) as u16;
    let mc3_err_valid: u16 = if (sts >> 31) & 1 != 0 { 1000 } else { 0 };
    let mc3_uc_err: u16 = if (sts >> 29) & 1 != 0 { 1000 } else { 0 };
    let composite = (mc3_ctl_en as u32/3).saturating_add(mc3_err_valid as u32/3).saturating_add(mc3_uc_err as u32/3);
    let mut s = MODULE.lock();
    let mc3_ema = ((s.mc3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mc3_ctl_en=mc3_ctl_en; s.mc3_err_valid=mc3_err_valid; s.mc3_uc_err=mc3_uc_err; s.mc3_ema=mc3_ema;
    serial_println!("[msr_ia32_mc3_ctl_status] age={} ctl={} valid={} uc={} ema={}", age, mc3_ctl_en, mc3_err_valid, mc3_uc_err, mc3_ema);
}
pub fn get_mc3_ctl_en()    -> u16 { MODULE.lock().mc3_ctl_en }
pub fn get_mc3_err_valid() -> u16 { MODULE.lock().mc3_err_valid }
pub fn get_mc3_uc_err()    -> u16 { MODULE.lock().mc3_uc_err }
pub fn get_mc3_ema()       -> u16 { MODULE.lock().mc3_ema }

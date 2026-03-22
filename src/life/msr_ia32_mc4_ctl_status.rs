#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc4_ctl_en: u16, mc4_err_valid: u16, mc4_uc_err: u16, mc4_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc4_ctl_en:0, mc4_err_valid:0, mc4_uc_err:0, mc4_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mc4_ctl_status] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let ctl: u32; let sts: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x410u32, out("eax") ctl, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x411u32, out("eax") sts, out("edx") _, options(nostack, nomem));
    }
    let mc4_ctl_en = ((ctl & 0xFF) * 1000 / 255) as u16;
    let mc4_err_valid: u16 = if (sts >> 31) & 1 != 0 { 1000 } else { 0 };
    let mc4_uc_err: u16 = if (sts >> 29) & 1 != 0 { 1000 } else { 0 };
    let composite = (mc4_ctl_en as u32/3).saturating_add(mc4_err_valid as u32/3).saturating_add(mc4_uc_err as u32/3);
    let mut s = MODULE.lock();
    let mc4_ema = ((s.mc4_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mc4_ctl_en=mc4_ctl_en; s.mc4_err_valid=mc4_err_valid; s.mc4_uc_err=mc4_uc_err; s.mc4_ema=mc4_ema;
    serial_println!("[msr_ia32_mc4_ctl_status] age={} ctl={} valid={} uc={} ema={}", age, mc4_ctl_en, mc4_err_valid, mc4_uc_err, mc4_ema);
}
pub fn get_mc4_ctl_en()    -> u16 { MODULE.lock().mc4_ctl_en }
pub fn get_mc4_err_valid() -> u16 { MODULE.lock().mc4_err_valid }
pub fn get_mc4_uc_err()    -> u16 { MODULE.lock().mc4_uc_err }
pub fn get_mc4_ema()       -> u16 { MODULE.lock().mc4_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mc1_ctl_en: u16, mc1_err_valid: u16, mc1_uc_err: u16, mc1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mc1_ctl_en:0, mc1_err_valid:0, mc1_uc_err:0, mc1_ema:0 });

pub fn init() { serial_println!("[msr_ia32_mc1] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let ctl: u32; let sts: u32;
    unsafe {
        asm!("rdmsr", in("ecx") 0x404u32, out("eax") ctl, out("edx") _, options(nostack, nomem));
        asm!("rdmsr", in("ecx") 0x405u32, out("eax") sts, out("edx") _, options(nostack, nomem));
    }
    // CTL: lower bits = error enable mask density
    let mc1_ctl_en = ((ctl & 0xFF) * 1000 / 255) as u16;
    // STATUS bit 63 (read as hi bit 31): VAL — error recorded
    let mc1_err_valid: u16 = if (sts >> 31) & 1 != 0 { 1000 } else { 0 };
    // STATUS bit 61 (hi bit 29): UC — uncorrectable error
    let mc1_uc_err: u16 = if (sts >> 29) & 1 != 0 { 1000 } else { 0 };
    let composite = (mc1_ctl_en as u32/3).saturating_add(mc1_err_valid as u32/3).saturating_add(mc1_uc_err as u32/3);
    let mut s = MODULE.lock();
    let mc1_ema = ((s.mc1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mc1_ctl_en=mc1_ctl_en; s.mc1_err_valid=mc1_err_valid; s.mc1_uc_err=mc1_uc_err; s.mc1_ema=mc1_ema;
    serial_println!("[msr_ia32_mc1] age={} ctl={} valid={} uc={} ema={}", age, mc1_ctl_en, mc1_err_valid, mc1_uc_err, mc1_ema);
}
pub fn get_mc1_ctl_en()    -> u16 { MODULE.lock().mc1_ctl_en }
pub fn get_mc1_err_valid() -> u16 { MODULE.lock().mc1_err_valid }
pub fn get_mc1_uc_err()    -> u16 { MODULE.lock().mc1_uc_err }
pub fn get_mc1_ema()       -> u16 { MODULE.lock().mc1_ema }

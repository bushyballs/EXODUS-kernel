#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel2_usr: u16, evtsel2_os: u16, evtsel2_en: u16, evtsel2_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel2_usr:0, evtsel2_os:0, evtsel2_en:0, evtsel2_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perfevtsel2] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x188u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 16: USR — count user-mode events
    let evtsel2_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    // bit 17: OS — count kernel-mode events
    let evtsel2_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    // bit 22: EN — counter enable
    let evtsel2_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel2_usr as u32/3).saturating_add(evtsel2_os as u32/3).saturating_add(evtsel2_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel2_ema = ((s.evtsel2_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel2_usr=evtsel2_usr; s.evtsel2_os=evtsel2_os; s.evtsel2_en=evtsel2_en; s.evtsel2_ema=evtsel2_ema;
    serial_println!("[msr_ia32_perfevtsel2] age={} usr={} os={} en={} ema={}", age, evtsel2_usr, evtsel2_os, evtsel2_en, evtsel2_ema);
}
pub fn get_evtsel2_usr() -> u16 { MODULE.lock().evtsel2_usr }
pub fn get_evtsel2_os()  -> u16 { MODULE.lock().evtsel2_os }
pub fn get_evtsel2_en()  -> u16 { MODULE.lock().evtsel2_en }
pub fn get_evtsel2_ema() -> u16 { MODULE.lock().evtsel2_ema }

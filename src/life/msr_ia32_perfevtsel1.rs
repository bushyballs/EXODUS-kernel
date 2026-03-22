#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel1_usr: u16, evtsel1_os: u16, evtsel1_en: u16, evtsel1_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel1_usr:0, evtsel1_os:0, evtsel1_en:0, evtsel1_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perfevtsel1] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x187u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 16: USR — count user-mode events
    let evtsel1_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    // bit 17: OS — count kernel-mode events
    let evtsel1_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    // bit 22: EN — counter enable
    let evtsel1_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel1_usr as u32/3).saturating_add(evtsel1_os as u32/3).saturating_add(evtsel1_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel1_ema = ((s.evtsel1_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel1_usr=evtsel1_usr; s.evtsel1_os=evtsel1_os; s.evtsel1_en=evtsel1_en; s.evtsel1_ema=evtsel1_ema;
    serial_println!("[msr_ia32_perfevtsel1] age={} usr={} os={} en={} ema={}", age, evtsel1_usr, evtsel1_os, evtsel1_en, evtsel1_ema);
}
pub fn get_evtsel1_usr() -> u16 { MODULE.lock().evtsel1_usr }
pub fn get_evtsel1_os()  -> u16 { MODULE.lock().evtsel1_os }
pub fn get_evtsel1_en()  -> u16 { MODULE.lock().evtsel1_en }
pub fn get_evtsel1_ema() -> u16 { MODULE.lock().evtsel1_ema }

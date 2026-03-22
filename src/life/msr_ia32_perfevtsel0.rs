#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel0_usr: u16, evtsel0_os: u16, evtsel0_en: u16, evtsel0_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel0_usr:0, evtsel0_os:0, evtsel0_en:0, evtsel0_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perfevtsel0] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x186u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 16: USR — count user-mode events
    let evtsel0_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    // bit 17: OS — count kernel-mode events
    let evtsel0_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    // bit 22: EN — counter enable
    let evtsel0_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel0_usr as u32/3).saturating_add(evtsel0_os as u32/3).saturating_add(evtsel0_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel0_ema = ((s.evtsel0_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel0_usr=evtsel0_usr; s.evtsel0_os=evtsel0_os; s.evtsel0_en=evtsel0_en; s.evtsel0_ema=evtsel0_ema;
    serial_println!("[msr_ia32_perfevtsel0] age={} usr={} os={} en={} ema={}", age, evtsel0_usr, evtsel0_os, evtsel0_en, evtsel0_ema);
}
pub fn get_evtsel0_usr() -> u16 { MODULE.lock().evtsel0_usr }
pub fn get_evtsel0_os()  -> u16 { MODULE.lock().evtsel0_os }
pub fn get_evtsel0_en()  -> u16 { MODULE.lock().evtsel0_en }
pub fn get_evtsel0_ema() -> u16 { MODULE.lock().evtsel0_ema }

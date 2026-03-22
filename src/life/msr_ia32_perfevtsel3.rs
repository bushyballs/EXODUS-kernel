#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel3_usr: u16, evtsel3_os: u16, evtsel3_en: u16, evtsel3_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel3_usr:0, evtsel3_os:0, evtsel3_en:0, evtsel3_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perfevtsel3] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x189u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 16: USR — count user-mode events
    let evtsel3_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    // bit 17: OS — count kernel-mode events
    let evtsel3_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    // bit 22: EN — counter enable
    let evtsel3_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel3_usr as u32/3).saturating_add(evtsel3_os as u32/3).saturating_add(evtsel3_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel3_ema = ((s.evtsel3_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel3_usr=evtsel3_usr; s.evtsel3_os=evtsel3_os; s.evtsel3_en=evtsel3_en; s.evtsel3_ema=evtsel3_ema;
    serial_println!("[msr_ia32_perfevtsel3] age={} usr={} os={} en={} ema={}", age, evtsel3_usr, evtsel3_os, evtsel3_en, evtsel3_ema);
}
pub fn get_evtsel3_usr() -> u16 { MODULE.lock().evtsel3_usr }
pub fn get_evtsel3_os()  -> u16 { MODULE.lock().evtsel3_os }
pub fn get_evtsel3_en()  -> u16 { MODULE.lock().evtsel3_en }
pub fn get_evtsel3_ema() -> u16 { MODULE.lock().evtsel3_ema }

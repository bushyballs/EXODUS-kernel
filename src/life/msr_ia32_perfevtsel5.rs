#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel5_usr: u16, evtsel5_os: u16, evtsel5_en: u16, evtsel5_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel5_usr:0, evtsel5_os:0, evtsel5_en:0, evtsel5_ema:0 });
pub fn init() { serial_println!("[msr_ia32_perfevtsel5] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x18Bu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let evtsel5_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let evtsel5_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    let evtsel5_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel5_usr as u32/3).saturating_add(evtsel5_os as u32/3).saturating_add(evtsel5_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel5_ema = ((s.evtsel5_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel5_usr=evtsel5_usr; s.evtsel5_os=evtsel5_os; s.evtsel5_en=evtsel5_en; s.evtsel5_ema=evtsel5_ema;
    serial_println!("[msr_ia32_perfevtsel5] age={} usr={} os={} en={} ema={}", age, evtsel5_usr, evtsel5_os, evtsel5_en, evtsel5_ema);
}
pub fn get_evtsel5_usr() -> u16 { MODULE.lock().evtsel5_usr }
pub fn get_evtsel5_os()  -> u16 { MODULE.lock().evtsel5_os }
pub fn get_evtsel5_en()  -> u16 { MODULE.lock().evtsel5_en }
pub fn get_evtsel5_ema() -> u16 { MODULE.lock().evtsel5_ema }

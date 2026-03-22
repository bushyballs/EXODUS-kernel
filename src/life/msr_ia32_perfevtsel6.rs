#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel6_usr: u16, evtsel6_os: u16, evtsel6_en: u16, evtsel6_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel6_usr:0, evtsel6_os:0, evtsel6_en:0, evtsel6_ema:0 });
pub fn init() { serial_println!("[msr_ia32_perfevtsel6] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x18Cu32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let evtsel6_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let evtsel6_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    let evtsel6_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel6_usr as u32/3).saturating_add(evtsel6_os as u32/3).saturating_add(evtsel6_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel6_ema = ((s.evtsel6_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel6_usr=evtsel6_usr; s.evtsel6_os=evtsel6_os; s.evtsel6_en=evtsel6_en; s.evtsel6_ema=evtsel6_ema;
    serial_println!("[msr_ia32_perfevtsel6] age={} usr={} os={} en={} ema={}", age, evtsel6_usr, evtsel6_os, evtsel6_en, evtsel6_ema);
}
pub fn get_evtsel6_usr() -> u16 { MODULE.lock().evtsel6_usr }
pub fn get_evtsel6_os()  -> u16 { MODULE.lock().evtsel6_os }
pub fn get_evtsel6_en()  -> u16 { MODULE.lock().evtsel6_en }
pub fn get_evtsel6_ema() -> u16 { MODULE.lock().evtsel6_ema }

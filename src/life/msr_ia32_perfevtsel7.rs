#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel7_usr: u16, evtsel7_os: u16, evtsel7_en: u16, evtsel7_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel7_usr:0, evtsel7_os:0, evtsel7_en:0, evtsel7_ema:0 });
pub fn init() { serial_println!("[msr_ia32_perfevtsel7] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x18Du32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let evtsel7_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let evtsel7_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    let evtsel7_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel7_usr as u32/3).saturating_add(evtsel7_os as u32/3).saturating_add(evtsel7_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel7_ema = ((s.evtsel7_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel7_usr=evtsel7_usr; s.evtsel7_os=evtsel7_os; s.evtsel7_en=evtsel7_en; s.evtsel7_ema=evtsel7_ema;
    serial_println!("[msr_ia32_perfevtsel7] age={} usr={} os={} en={} ema={}", age, evtsel7_usr, evtsel7_os, evtsel7_en, evtsel7_ema);
}
pub fn get_evtsel7_usr() -> u16 { MODULE.lock().evtsel7_usr }
pub fn get_evtsel7_os()  -> u16 { MODULE.lock().evtsel7_os }
pub fn get_evtsel7_en()  -> u16 { MODULE.lock().evtsel7_en }
pub fn get_evtsel7_ema() -> u16 { MODULE.lock().evtsel7_ema }

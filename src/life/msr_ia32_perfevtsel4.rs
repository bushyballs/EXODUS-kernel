#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { evtsel4_usr: u16, evtsel4_os: u16, evtsel4_en: u16, evtsel4_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { evtsel4_usr:0, evtsel4_os:0, evtsel4_en:0, evtsel4_ema:0 });
pub fn init() { serial_println!("[msr_ia32_perfevtsel4] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x18Au32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let evtsel4_usr: u16 = if (lo >> 16) & 1 != 0 { 1000 } else { 0 };
    let evtsel4_os: u16 = if (lo >> 17) & 1 != 0 { 1000 } else { 0 };
    let evtsel4_en: u16 = if (lo >> 22) & 1 != 0 { 1000 } else { 0 };
    let composite = (evtsel4_usr as u32/3).saturating_add(evtsel4_os as u32/3).saturating_add(evtsel4_en as u32/3);
    let mut s = MODULE.lock();
    let evtsel4_ema = ((s.evtsel4_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.evtsel4_usr=evtsel4_usr; s.evtsel4_os=evtsel4_os; s.evtsel4_en=evtsel4_en; s.evtsel4_ema=evtsel4_ema;
    serial_println!("[msr_ia32_perfevtsel4] age={} usr={} os={} en={} ema={}", age, evtsel4_usr, evtsel4_os, evtsel4_en, evtsel4_ema);
}
pub fn get_evtsel4_usr() -> u16 { MODULE.lock().evtsel4_usr }
pub fn get_evtsel4_os()  -> u16 { MODULE.lock().evtsel4_os }
pub fn get_evtsel4_en()  -> u16 { MODULE.lock().evtsel4_en }
pub fn get_evtsel4_ema() -> u16 { MODULE.lock().evtsel4_ema }

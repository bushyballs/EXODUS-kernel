#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { deadline_set: u16, deadline_delta: u16, timer_activity: u16, deadline_ema: u16, last_lo: u32 }
static MODULE: Mutex<State> = Mutex::new(State { deadline_set:0, deadline_delta:0, timer_activity:0, deadline_ema:0, last_lo:0 });

pub fn init() { serial_println!("[msr_ia32_tsc_deadline] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x6E0u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let deadline_set: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let delta = lo.wrapping_sub(MODULE.lock().last_lo);
    let deadline_delta = (delta / 4096).min(1000) as u16;
    let timer_activity = deadline_set;
    let mut s = MODULE.lock();
    let deadline_ema = ((s.deadline_ema as u32).wrapping_mul(7).saturating_add(timer_activity as u32)/8).min(1000) as u16;
    s.last_lo=lo; s.deadline_set=deadline_set; s.deadline_delta=deadline_delta; s.timer_activity=timer_activity; s.deadline_ema=deadline_ema;
    serial_println!("[msr_ia32_tsc_deadline] age={} set={} delta={} act={} ema={}", age, deadline_set, deadline_delta, timer_activity, deadline_ema);
}
pub fn get_deadline_set()    -> u16 { MODULE.lock().deadline_set }
pub fn get_deadline_delta()  -> u16 { MODULE.lock().deadline_delta }
pub fn get_timer_activity()  -> u16 { MODULE.lock().timer_activity }
pub fn get_deadline_ema()    -> u16 { MODULE.lock().deadline_ema }

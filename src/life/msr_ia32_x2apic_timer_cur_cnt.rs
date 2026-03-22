#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { timer_cur: u16, timer_progress: u16, timer_delta: u16, timer_cur_ema: u16, last_cur: u32 }
static MODULE: Mutex<State> = Mutex::new(State { timer_cur:0, timer_progress:0, timer_delta:0, timer_cur_ema:0, last_cur:0 });

#[inline]
fn has_x2apic() -> bool {
    let ecx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") ecx, lateout("edx") _, options(nostack,nomem)); }
    (ecx >> 21) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_x2apic_timer_cur_cnt] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    if !has_x2apic() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x839u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let timer_cur: u16 = if lo != 0 { 1000 } else { 0 };
    let timer_progress = (lo >> 22).min(1000) as u16;
    let mut s = MODULE.lock();
    let delta_raw = s.last_cur.wrapping_sub(lo);
    let timer_delta = (delta_raw >> 12).min(1000) as u16;
    let timer_cur_ema = ((s.timer_cur_ema as u32).wrapping_mul(7).saturating_add(timer_cur as u32)/8).min(1000) as u16;
    s.last_cur=lo; s.timer_cur=timer_cur; s.timer_progress=timer_progress; s.timer_delta=timer_delta; s.timer_cur_ema=timer_cur_ema;
    serial_println!("[msr_ia32_x2apic_timer_cur_cnt] age={} cur={} prog={} delta={} ema={}", age, timer_cur, timer_progress, timer_delta, timer_cur_ema);
}
pub fn get_timer_cur()      -> u16 { MODULE.lock().timer_cur }
pub fn get_timer_progress() -> u16 { MODULE.lock().timer_progress }
pub fn get_timer_delta()    -> u16 { MODULE.lock().timer_delta }
pub fn get_timer_cur_ema()  -> u16 { MODULE.lock().timer_cur_ema }

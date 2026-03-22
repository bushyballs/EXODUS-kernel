#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { target_fid: u16, ida_engage: u16, perf_ctl_active: u16, perf_ctl_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { target_fid:0, ida_engage:0, perf_ctl_active:0, perf_ctl_ema:0 });

pub fn init() { serial_println!("[msr_ia32_perf_ctl] init"); }
pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x199u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let fid_raw = lo & 0xFF;
    let target_fid = ((fid_raw * 1000) / 255).min(1000) as u16;
    let perf_ctl_active: u16 = if fid_raw != 0 { 1000 } else { 0 };
    let ida_engage: u16 = if (lo >> 32) & 1 != 0 { 1000 } else { 0 };
    let composite = (target_fid as u32/3).saturating_add(perf_ctl_active as u32/3).saturating_add(ida_engage as u32/3);
    let mut s = MODULE.lock();
    let perf_ctl_ema = ((s.perf_ctl_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.target_fid=target_fid; s.ida_engage=ida_engage; s.perf_ctl_active=perf_ctl_active; s.perf_ctl_ema=perf_ctl_ema;
    serial_println!("[msr_ia32_perf_ctl] age={} fid={} ida={} active={} ema={}", age, target_fid, ida_engage, perf_ctl_active, perf_ctl_ema);
}
pub fn get_target_fid()       -> u16 { MODULE.lock().target_fid }
pub fn get_ida_engage()       -> u16 { MODULE.lock().ida_engage }
pub fn get_perf_ctl_active()  -> u16 { MODULE.lock().perf_ctl_active }
pub fn get_perf_ctl_ema()     -> u16 { MODULE.lock().perf_ctl_ema }

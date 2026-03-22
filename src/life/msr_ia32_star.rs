#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { syscall_cs_valid: u16, sysret_cs_valid: u16, star_configured: u16, star_topology_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { syscall_cs_valid:0, sysret_cs_valid:0, star_configured:0, star_topology_ema:0 });

pub fn init() { serial_println!("[msr_ia32_star] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xC0000081u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    let syscall_cs = (hi & 0xFFFF) as u16; let sysret_cs = ((hi >> 16) & 0xFFFF) as u16; let _ = lo;
    let syscall_cs_valid: u16 = if syscall_cs != 0 { 1000 } else { 0 };
    let sysret_cs_valid: u16  = if sysret_cs  != 0 { 1000 } else { 0 };
    let star_configured: u16  = if syscall_cs != 0 && sysret_cs != 0 { 1000 } else { 0 };
    let composite = (syscall_cs_valid as u32/3).saturating_add(sysret_cs_valid as u32/3).saturating_add(star_configured as u32/3);
    let mut s = MODULE.lock();
    let star_topology_ema = ((s.star_topology_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.syscall_cs_valid=syscall_cs_valid; s.sysret_cs_valid=sysret_cs_valid; s.star_configured=star_configured; s.star_topology_ema=star_topology_ema;
    serial_println!("[msr_ia32_star] age={} syscall_cs={} sysret_cs={} cfg={} ema={}", age, syscall_cs_valid, sysret_cs_valid, star_configured, star_topology_ema);
}
pub fn get_syscall_cs_valid() -> u16 { MODULE.lock().syscall_cs_valid }
pub fn get_sysret_cs_valid()  -> u16 { MODULE.lock().sysret_cs_valid }
pub fn get_star_configured()  -> u16 { MODULE.lock().star_configured }
pub fn get_star_topology_ema()-> u16 { MODULE.lock().star_topology_ema }

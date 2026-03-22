#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pebs_enable_gp: u16, pebs_enable_fixed: u16, pebs_active: u16, pebs_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pebs_enable_gp:0, pebs_enable_fixed:0, pebs_active:0, pebs_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pebs_enable] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    // Check PEBS via CPUID 1 EDX bit 21 (DS) + PERF_CAP
    let edx: u32;
    unsafe { asm!("push rbx", "cpuid", "pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack, nomem)); }
    if (edx >> 21) & 1 == 0 { return; }
    let lo: u32; let hi: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x3F1u32, out("eax") lo, out("edx") hi, options(nostack, nomem)); }
    // lo[3:0]: GP counters 0-3 PEBS enable
    let pebs_enable_gp = ((lo & 0xF) * 1000 / 15) as u16;
    // hi[2:0]: fixed counters 0-2 PEBS enable
    let pebs_enable_fixed = ((hi & 0x7) * 1000 / 7) as u16;
    let pebs_active: u16 = if lo != 0 || hi != 0 { 1000 } else { 0 };
    let composite = (pebs_enable_gp as u32/3).saturating_add(pebs_enable_fixed as u32/3).saturating_add(pebs_active as u32/3);
    let mut s = MODULE.lock();
    let pebs_ema = ((s.pebs_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pebs_enable_gp=pebs_enable_gp; s.pebs_enable_fixed=pebs_enable_fixed; s.pebs_active=pebs_active; s.pebs_ema=pebs_ema;
    serial_println!("[msr_ia32_pebs_enable] age={} gp={} fixed={} active={} ema={}", age, pebs_enable_gp, pebs_enable_fixed, pebs_active, pebs_ema);
}
pub fn get_pebs_enable_gp()    -> u16 { MODULE.lock().pebs_enable_gp }
pub fn get_pebs_enable_fixed() -> u16 { MODULE.lock().pebs_enable_fixed }
pub fn get_pebs_active()       -> u16 { MODULE.lock().pebs_active }
pub fn get_pebs_ema()          -> u16 { MODULE.lock().pebs_ema }

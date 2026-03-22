#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { mcg_count: u16, mcg_ctl_present: u16, mcg_ext_present: u16, mcg_cap_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { mcg_count:0, mcg_ctl_present:0, mcg_ext_present:0, mcg_cap_ema:0 });

#[inline]
fn has_mce() -> bool {
    let edx: u32;
    unsafe { asm!("push rbx","cpuid","pop rbx", inout("eax") 1u32 => _, lateout("ecx") _, lateout("edx") edx, options(nostack,nomem)); }
    (edx >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mcg_cap] init"); }
pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }
    if !has_mce() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x179u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    let count_raw = lo & 0xFF;
    let mcg_count = ((count_raw * 1000) / 32).min(1000) as u16;
    let mcg_ctl_present: u16 = if (lo >> 8) & 1 != 0 { 1000 } else { 0 };
    let mcg_ext_present: u16 = if (lo >> 9) & 1 != 0 { 1000 } else { 0 };
    let composite = (mcg_count as u32/3).saturating_add(mcg_ctl_present as u32/3).saturating_add(mcg_ext_present as u32/3);
    let mut s = MODULE.lock();
    let mcg_cap_ema = ((s.mcg_cap_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.mcg_count=mcg_count; s.mcg_ctl_present=mcg_ctl_present; s.mcg_ext_present=mcg_ext_present; s.mcg_cap_ema=mcg_cap_ema;
    serial_println!("[msr_ia32_mcg_cap] age={} count={} ctl_present={} ext={} ema={}", age, mcg_count, mcg_ctl_present, mcg_ext_present, mcg_cap_ema);
}
pub fn get_mcg_count()       -> u16 { MODULE.lock().mcg_count }
pub fn get_mcg_ctl_present() -> u16 { MODULE.lock().mcg_ctl_present }
pub fn get_mcg_ext_present() -> u16 { MODULE.lock().mcg_ext_present }
pub fn get_mcg_cap_ema()     -> u16 { MODULE.lock().mcg_cap_ema }

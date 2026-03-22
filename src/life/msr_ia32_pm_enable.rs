#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pm_hwp_enable: u16, pm_hwp_ema: u16, pm_pad0: u16, pm_pad1: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pm_hwp_enable:0, pm_hwp_ema:0, pm_pad0:0, pm_pad1:0 });

#[inline]
fn has_hwp() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 7) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_pm_enable] init"); }
pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_hwp() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x770u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: HWP_ENABLE — once set, cannot be cleared until reset
    let pm_hwp_enable: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let mut s = MODULE.lock();
    let pm_hwp_ema = ((s.pm_hwp_ema as u32).wrapping_mul(7).saturating_add(pm_hwp_enable as u32)/8).min(1000) as u16;
    s.pm_hwp_enable=pm_hwp_enable; s.pm_hwp_ema=pm_hwp_ema;
    serial_println!("[msr_ia32_pm_enable] age={} hwp_en={} ema={}", age, pm_hwp_enable, pm_hwp_ema);
}
pub fn get_pm_hwp_enable() -> u16 { MODULE.lock().pm_hwp_enable }
pub fn get_pm_hwp_ema()    -> u16 { MODULE.lock().pm_hwp_ema }

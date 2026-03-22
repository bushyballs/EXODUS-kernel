#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { pkg_hdc_active: u16, core_hdc_active: u16, hdc_stall: u16, hdc_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { pkg_hdc_active:0, core_hdc_active:0, hdc_stall:0, hdc_ema:0 });

pub fn init() { serial_println!("[msr_ia32_pkg_hdc_ctl] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    // Check HDC support (CPUID 6.0 EAX bit 13)
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    if (eax >> 13) & 1 == 0 { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0xDB0u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bit 0: HDC_PKG_Enable — package HDC arbiter enabled
    let pkg_hdc_active: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: Core_HDC_Enable — per-core HDC enabled
    let core_hdc_active: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // Both active = stall injection occurring (power throttle)
    let hdc_stall: u16 = if (lo & 3) == 3 { 1000 } else { (lo & 3) as u16 * 500 };
    let composite = (pkg_hdc_active as u32/3).saturating_add(core_hdc_active as u32/3).saturating_add(hdc_stall as u32/3);
    let mut s = MODULE.lock();
    let hdc_ema = ((s.hdc_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.pkg_hdc_active=pkg_hdc_active; s.core_hdc_active=core_hdc_active; s.hdc_stall=hdc_stall; s.hdc_ema=hdc_ema;
    serial_println!("[msr_ia32_pkg_hdc_ctl] age={} pkg={} core={} stall={} ema={}", age, pkg_hdc_active, core_hdc_active, hdc_stall, hdc_ema);
}
pub fn get_pkg_hdc_active()  -> u16 { MODULE.lock().pkg_hdc_active }
pub fn get_core_hdc_active() -> u16 { MODULE.lock().core_hdc_active }
pub fn get_hdc_stall()       -> u16 { MODULE.lock().hdc_stall }
pub fn get_hdc_ema()         -> u16 { MODULE.lock().hdc_ema }

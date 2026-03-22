#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State { hwp_pkg_min: u16, hwp_pkg_max: u16, hwp_pkg_desired: u16, hwp_pkg_ema: u16 }
static MODULE: Mutex<State> = Mutex::new(State { hwp_pkg_min:0, hwp_pkg_max:0, hwp_pkg_desired:0, hwp_pkg_ema:0 });

#[inline]
fn has_hwp_pkg() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    // bit 11: HWP Package Level Request
    (eax >> 11) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_hwp_request_pkg] init"); }
pub fn tick(age: u32) {
    if age % 3000 != 0 { return; }
    if !has_hwp_pkg() { return; }
    let lo: u32;
    unsafe { asm!("rdmsr", in("ecx") 0x772u32, out("eax") lo, out("edx") _, options(nostack, nomem)); }
    // bits[7:0]: Minimum Performance
    let hwp_pkg_min = ((lo & 0xFF) * 1000 / 255) as u16;
    // bits[15:8]: Maximum Performance
    let hwp_pkg_max = (((lo >> 8) & 0xFF) * 1000 / 255) as u16;
    // bits[23:16]: Desired Performance (0=HW autonomy)
    let raw_des = (lo >> 16) & 0xFF;
    let hwp_pkg_desired = if raw_des == 0 { 0u16 } else { ((raw_des * 1000) / 255) as u16 };
    let composite = (hwp_pkg_min as u32/3).saturating_add(hwp_pkg_max as u32/3).saturating_add(hwp_pkg_desired as u32/3);
    let mut s = MODULE.lock();
    let hwp_pkg_ema = ((s.hwp_pkg_ema as u32).wrapping_mul(7).saturating_add(composite)/8).min(1000) as u16;
    s.hwp_pkg_min=hwp_pkg_min; s.hwp_pkg_max=hwp_pkg_max; s.hwp_pkg_desired=hwp_pkg_desired; s.hwp_pkg_ema=hwp_pkg_ema;
    serial_println!("[msr_ia32_hwp_request_pkg] age={} min={} max={} des={} ema={}", age, hwp_pkg_min, hwp_pkg_max, hwp_pkg_desired, hwp_pkg_ema);
}
pub fn get_hwp_pkg_min()     -> u16 { MODULE.lock().hwp_pkg_min }
pub fn get_hwp_pkg_max()     -> u16 { MODULE.lock().hwp_pkg_max }
pub fn get_hwp_pkg_desired() -> u16 { MODULE.lock().hwp_pkg_desired }
pub fn get_hwp_pkg_ema()     -> u16 { MODULE.lock().hwp_pkg_ema }

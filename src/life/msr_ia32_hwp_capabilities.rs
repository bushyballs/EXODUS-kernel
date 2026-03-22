#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hwp_highest_perf: u16,
    hwp_guaranteed_perf: u16,
    hwp_most_efficient_perf: u16,
    hwp_cap_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hwp_highest_perf: 0,
    hwp_guaranteed_perf: 0,
    hwp_most_efficient_perf: 0,
    hwp_cap_ema: 0,
});

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

pub fn init() { serial_println!("[msr_ia32_hwp_capabilities] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_hwp() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x771u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let raw_hi = lo & 0xFF;
    let hwp_highest_perf = ((raw_hi * 1000) / 255).min(1000) as u16;
    let raw_guar = (lo >> 8) & 0xFF;
    let hwp_guaranteed_perf = ((raw_guar * 1000) / 255).min(1000) as u16;
    let raw_eff = (lo >> 16) & 0xFF;
    let hwp_most_efficient_perf = ((raw_eff * 1000) / 255).min(1000) as u16;

    let composite = (hwp_highest_perf as u32 / 3)
        .saturating_add(hwp_guaranteed_perf as u32 / 3)
        .saturating_add(hwp_most_efficient_perf as u32 / 3);

    let mut s = MODULE.lock();
    let hwp_cap_ema = ((s.hwp_cap_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.hwp_highest_perf = hwp_highest_perf;
    s.hwp_guaranteed_perf = hwp_guaranteed_perf;
    s.hwp_most_efficient_perf = hwp_most_efficient_perf;
    s.hwp_cap_ema = hwp_cap_ema;

    serial_println!("[msr_ia32_hwp_capabilities] age={} hi={} guar={} eff={} ema={}",
        age, hwp_highest_perf, hwp_guaranteed_perf, hwp_most_efficient_perf, hwp_cap_ema);
}

pub fn get_hwp_highest_perf()        -> u16 { MODULE.lock().hwp_highest_perf }
pub fn get_hwp_guaranteed_perf()     -> u16 { MODULE.lock().hwp_guaranteed_perf }
pub fn get_hwp_most_efficient_perf() -> u16 { MODULE.lock().hwp_most_efficient_perf }
pub fn get_hwp_cap_ema()             -> u16 { MODULE.lock().hwp_cap_ema }

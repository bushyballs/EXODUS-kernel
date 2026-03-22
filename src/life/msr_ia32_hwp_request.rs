#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hwp_min_perf: u16,
    hwp_max_perf: u16,
    hwp_desired_perf: u16,
    msr_ia32_hwp_request_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hwp_min_perf: 0,
    hwp_max_perf: 0,
    hwp_desired_perf: 0,
    msr_ia32_hwp_request_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_hwp_request] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x774u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let hwp_min_perf = ((lo & 0xFF) * 1000 / 255).min(1000) as u16;
    let hwp_max_perf = (((lo >> 8) & 0xFF) * 1000 / 255).min(1000) as u16;
    let hwp_desired_perf = (((lo >> 16) & 0xFF) * 1000 / 255).min(1000) as u16;

    let composite = (hwp_min_perf as u32 / 3)
        .saturating_add(hwp_max_perf as u32 / 3)
        .saturating_add(hwp_desired_perf as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_hwp_request_ema = ((s.msr_ia32_hwp_request_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.hwp_min_perf = hwp_min_perf;
    s.hwp_max_perf = hwp_max_perf;
    s.hwp_desired_perf = hwp_desired_perf;
    s.msr_ia32_hwp_request_ema = msr_ia32_hwp_request_ema;

    serial_println!("[msr_ia32_hwp_request] age={} min={} max={} des={} ema={}",
        age, hwp_min_perf, hwp_max_perf, hwp_desired_perf, msr_ia32_hwp_request_ema);
}

pub fn get_hwp_min_perf()    -> u16 { MODULE.lock().hwp_min_perf }
pub fn get_hwp_max_perf()    -> u16 { MODULE.lock().hwp_max_perf }
pub fn get_hwp_desired_perf()    -> u16 { MODULE.lock().hwp_desired_perf }
pub fn get_msr_ia32_hwp_request_ema() -> u16 { MODULE.lock().msr_ia32_hwp_request_ema }

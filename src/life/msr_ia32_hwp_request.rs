#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hwp_min_perf: u16,
    hwp_max_perf: u16,
    hwp_desired: u16,
    hwp_energy_pref: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hwp_min_perf: 0,
    hwp_max_perf: 0,
    hwp_desired: 0,
    hwp_energy_pref: 0,
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

pub fn init() { serial_println!("[msr_ia32_hwp_request] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }
    if !has_hwp() { return; }

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

    // Each field is 8 bits (0-255), scale to 0-1000
    let min_raw  = lo & 0xFF;
    let max_raw  = (lo >> 8) & 0xFF;
    let des_raw  = (lo >> 16) & 0xFF;
    let epp_raw  = (lo >> 24) & 0xFF;

    let hwp_min_perf  = ((min_raw  * 1000) / 255) as u16;
    let hwp_max_perf  = ((max_raw  * 1000) / 255) as u16;
    let hwp_desired   = ((des_raw  * 1000) / 255) as u16;
    // EPP: 0=max perf, 255=max efficiency — invert so 1000 = performance-oriented
    let hwp_energy_pref = (((255 - epp_raw) * 1000) / 255) as u16;

    let mut s = MODULE.lock();
    s.hwp_min_perf  = hwp_min_perf;
    s.hwp_max_perf  = hwp_max_perf;
    s.hwp_desired   = hwp_desired;
    s.hwp_energy_pref = hwp_energy_pref;

    serial_println!("[msr_ia32_hwp_request] age={} min={} max={} desired={} epref={}",
        age, hwp_min_perf, hwp_max_perf, hwp_desired, hwp_energy_pref);
}

pub fn get_hwp_min_perf()   -> u16 { MODULE.lock().hwp_min_perf }
pub fn get_hwp_max_perf()   -> u16 { MODULE.lock().hwp_max_perf }
pub fn get_hwp_desired()    -> u16 { MODULE.lock().hwp_desired }
pub fn get_hwp_energy_pref() -> u16 { MODULE.lock().hwp_energy_pref }

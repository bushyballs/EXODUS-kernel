#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hwp_guaranteed_change: u16,
    hwp_excursion: u16,
    hwp_activity: u16,
    hwp_status_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hwp_guaranteed_change: 0,
    hwp_excursion: 0,
    hwp_activity: 0,
    hwp_status_ema: 0,
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

pub fn init() { serial_println!("[msr_ia32_hwp_status] init"); }

pub fn tick(age: u32) {
    if age % 1000 != 0 { return; }
    if !has_hwp() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x777u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    let hwp_guaranteed_change: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    let hwp_excursion: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };
    let hwp_activity: u16 = if lo != 0 { 1000 } else { 0 };

    let composite = (hwp_guaranteed_change as u32 / 3)
        .saturating_add(hwp_excursion as u32 / 3)
        .saturating_add(hwp_activity as u32 / 3);

    let mut s = MODULE.lock();
    let hwp_status_ema = ((s.hwp_status_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.hwp_guaranteed_change = hwp_guaranteed_change;
    s.hwp_excursion = hwp_excursion;
    s.hwp_activity = hwp_activity;
    s.hwp_status_ema = hwp_status_ema;

    serial_println!("[msr_ia32_hwp_status] age={} guar_chg={} excur={} active={} ema={}",
        age, hwp_guaranteed_change, hwp_excursion, hwp_activity, hwp_status_ema);
}

pub fn get_hwp_guaranteed_change() -> u16 { MODULE.lock().hwp_guaranteed_change }
pub fn get_hwp_excursion()         -> u16 { MODULE.lock().hwp_excursion }
pub fn get_hwp_activity()          -> u16 { MODULE.lock().hwp_activity }
pub fn get_hwp_status_ema()        -> u16 { MODULE.lock().hwp_status_ema }

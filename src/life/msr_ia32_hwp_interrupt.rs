#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    hwp_intr_guaranteed: u16,
    hwp_intr_excursion: u16,
    hwp_intr_enabled: u16,
    hwp_intr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    hwp_intr_guaranteed: 0,
    hwp_intr_excursion: 0,
    hwp_intr_enabled: 0,
    hwp_intr_ema: 0,
});

#[inline]
fn has_hwp_notification() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    (eax >> 8) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_hwp_interrupt] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_hwp_notification() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x773u32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: Guaranteed Performance Change interrupt enable
    let hwp_intr_guaranteed: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: Excursion Minimum interrupt enable
    let hwp_intr_excursion: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    let hwp_intr_enabled: u16 = if lo & 3 != 0 { 1000 } else { 0 };

    let composite = (hwp_intr_guaranteed as u32 / 3)
        .saturating_add(hwp_intr_excursion as u32 / 3)
        .saturating_add(hwp_intr_enabled as u32 / 3);

    let mut s = MODULE.lock();
    let hwp_intr_ema = ((s.hwp_intr_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.hwp_intr_guaranteed = hwp_intr_guaranteed;
    s.hwp_intr_excursion = hwp_intr_excursion;
    s.hwp_intr_enabled = hwp_intr_enabled;
    s.hwp_intr_ema = hwp_intr_ema;

    serial_println!("[msr_ia32_hwp_interrupt] age={} guar={} excur={} en={} ema={}",
        age, hwp_intr_guaranteed, hwp_intr_excursion, hwp_intr_enabled, hwp_intr_ema);
}

pub fn get_hwp_intr_guaranteed() -> u16 { MODULE.lock().hwp_intr_guaranteed }
pub fn get_hwp_intr_excursion()  -> u16 { MODULE.lock().hwp_intr_excursion }
pub fn get_hwp_intr_enabled()    -> u16 { MODULE.lock().hwp_intr_enabled }
pub fn get_hwp_intr_ema()        -> u16 { MODULE.lock().hwp_intr_ema }

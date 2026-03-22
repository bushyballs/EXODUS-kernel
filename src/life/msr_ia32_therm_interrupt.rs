#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    therm_intr_hi: u16,
    therm_intr_lo: u16,
    therm_intr_prochot: u16,
    therm_intr_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    therm_intr_hi: 0,
    therm_intr_lo: 0,
    therm_intr_prochot: 0,
    therm_intr_ema: 0,
});

#[inline]
fn has_dts() -> bool {
    let eax: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 6u32 => eax,
            lateout("ecx") _, lateout("edx") _,
            options(nostack, nomem),
        );
    }
    eax & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_therm_interrupt] init"); }

pub fn tick(age: u32) {
    if age % 5000 != 0 { return; }
    if !has_dts() { return; }

    let lo: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x19Du32,
            out("eax") lo,
            out("edx") _,
            options(nostack, nomem),
        );
    }

    // bit 0: High-temperature interrupt enable
    let therm_intr_hi: u16 = if (lo & 1) != 0 { 1000 } else { 0 };
    // bit 1: Low-temperature interrupt enable
    let therm_intr_lo: u16 = if (lo >> 1) & 1 != 0 { 1000 } else { 0 };
    // bit 2: PROCHOT interrupt enable
    let therm_intr_prochot: u16 = if (lo >> 2) & 1 != 0 { 1000 } else { 0 };

    let composite = (therm_intr_hi as u32 / 3)
        .saturating_add(therm_intr_lo as u32 / 3)
        .saturating_add(therm_intr_prochot as u32 / 3);

    let mut s = MODULE.lock();
    let therm_intr_ema = ((s.therm_intr_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.therm_intr_hi = therm_intr_hi;
    s.therm_intr_lo = therm_intr_lo;
    s.therm_intr_prochot = therm_intr_prochot;
    s.therm_intr_ema = therm_intr_ema;

    serial_println!("[msr_ia32_therm_interrupt] age={} hi={} lo={} prochot={} ema={}",
        age, therm_intr_hi, therm_intr_lo, therm_intr_prochot, therm_intr_ema);
}

pub fn get_therm_intr_hi()      -> u16 { MODULE.lock().therm_intr_hi }
pub fn get_therm_intr_lo()      -> u16 { MODULE.lock().therm_intr_lo }
pub fn get_therm_intr_prochot() -> u16 { MODULE.lock().therm_intr_prochot }
pub fn get_therm_intr_ema()     -> u16 { MODULE.lock().therm_intr_ema }

#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    tsc_dl_lo: u16,
    tsc_dl_hi: u16,
    tsc_dl_active: u16,
    msr_ia32_tsc_deadline_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    tsc_dl_lo: 0,
    tsc_dl_hi: 0,
    tsc_dl_active: 0,
    msr_ia32_tsc_deadline_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_tsc_deadline] init"); }

pub fn tick(age: u32) {
    if age % 2000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x6E0u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // TSC deadline for APIC timer
    let tsc_dl_lo = ((lo & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let tsc_dl_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let tsc_dl_active: u16 = if (lo != 0 || hi != 0) { 1000 } else { 0 };

    let composite = (tsc_dl_lo as u32 / 3)
        .saturating_add(tsc_dl_hi as u32 / 3)
        .saturating_add(tsc_dl_active as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_tsc_deadline_ema = ((s.msr_ia32_tsc_deadline_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.tsc_dl_lo = tsc_dl_lo;
    s.tsc_dl_hi = tsc_dl_hi;
    s.tsc_dl_active = tsc_dl_active;
    s.msr_ia32_tsc_deadline_ema = msr_ia32_tsc_deadline_ema;

    serial_println!("[msr_ia32_tsc_deadline] age={} tsc_dl_lo={} tsc_dl_hi={} tsc_dl_active={} ema={}",
        age, tsc_dl_lo, tsc_dl_hi, tsc_dl_active, msr_ia32_tsc_deadline_ema);
}

pub fn get_tsc_dl_lo()  -> u16 { MODULE.lock().tsc_dl_lo }
pub fn get_tsc_dl_hi()  -> u16 { MODULE.lock().tsc_dl_hi }
pub fn get_tsc_dl_active()  -> u16 { MODULE.lock().tsc_dl_active }
pub fn get_msr_ia32_tsc_deadline_ema() -> u16 { MODULE.lock().msr_ia32_tsc_deadline_ema }

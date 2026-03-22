#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

/// MSR 0x415 — IA32_MC5_STATUS (Machine Check Bank 5)
/// Signals: valid_error, uncorrectable, mca_active, mc5_ema
struct State {
    mc5_valid: u16,
    mc5_uncorrectable: u16,
    mc5_active: u16,
    mc5_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    mc5_valid: 0,
    mc5_uncorrectable: 0,
    mc5_active: 0,
    mc5_ema: 0,
});

#[inline]
fn has_mca() -> bool {
    let edx: u32;
    unsafe {
        asm!(
            "push rbx", "cpuid", "pop rbx",
            inout("eax") 1u32 => _,
            lateout("ecx") _,
            lateout("edx") edx,
            options(nostack, nomem),
        );
    }
    (edx >> 14) & 1 == 1
}

pub fn init() { serial_println!("[msr_ia32_mc5_status] init"); }

pub fn tick(age: u32) {
    if age % 4000 != 0 { return; }
    if !has_mca() { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x415u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // bit 63 (top of hi) = VAL — valid error
    let mc5_valid: u16 = if (hi >> 31) & 1 != 0 { 1000 } else { 0 };
    // bit 61 (hi bit 29) = UC — uncorrectable
    let mc5_uncorrectable: u16 = if (hi >> 29) & 1 != 0 { 1000 } else { 0 };
    // bit 60 (hi bit 28) = EN — error condition enabled
    let mc5_active: u16 = if (hi >> 28) & 1 != 0 { 500 } else { 0 };

    let composite: u16 = (mc5_valid / 4)
        .saturating_add(mc5_uncorrectable / 4)
        .saturating_add(mc5_active / 2);

    let mut s = MODULE.lock();
    let ema = ((s.mc5_ema as u32).wrapping_mul(7).saturating_add(composite as u32) / 8)
        .min(1000) as u16;
    s.mc5_valid = mc5_valid;
    s.mc5_uncorrectable = mc5_uncorrectable;
    s.mc5_active = mc5_active;
    s.mc5_ema = ema;

    serial_println!("[msr_ia32_mc5_status] age={} lo={:#010x} hi={:#010x} valid={} unc={} active={} ema={}",
        age, lo, hi, mc5_valid, mc5_uncorrectable, mc5_active, ema);
}

pub fn get_mc5_valid() -> u16 { MODULE.lock().mc5_valid }
pub fn get_mc5_uncorrectable() -> u16 { MODULE.lock().mc5_uncorrectable }
pub fn get_mc5_active() -> u16 { MODULE.lock().mc5_active }
pub fn get_mc5_ema() -> u16 { MODULE.lock().mc5_ema }

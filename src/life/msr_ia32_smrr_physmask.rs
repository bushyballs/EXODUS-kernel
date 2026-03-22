#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    smrr_valid: u16,
    smrr_mask_lo: u16,
    smrr_mask_hi: u16,
    smrr_mask_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    smrr_valid: 0,
    smrr_mask_lo: 0,
    smrr_mask_hi: 0,
    smrr_mask_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_smrr_physmask] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1F3u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // bit 11: SMRR valid (SMM region is enabled)
    let smrr_valid: u16 = if (lo >> 11) & 1 != 0 { 1000 } else { 0 };
    // bits[31:12]: mask low bits
    let smrr_mask_lo = (((lo >> 12) & 0xFFFF) * 1000 / 65535).min(1000) as u16;
    let smrr_mask_hi = ((hi & 0xFFFF) * 1000 / 65535).min(1000) as u16;

    let composite = (smrr_valid as u32 / 3)
        .saturating_add(smrr_mask_lo as u32 / 3)
        .saturating_add(smrr_mask_hi as u32 / 3);

    let mut s = MODULE.lock();
    let smrr_mask_ema = ((s.smrr_mask_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.smrr_valid = smrr_valid;
    s.smrr_mask_lo = smrr_mask_lo;
    s.smrr_mask_hi = smrr_mask_hi;
    s.smrr_mask_ema = smrr_mask_ema;

    serial_println!("[msr_ia32_smrr_physmask] age={} valid={} mask_lo={} mask_hi={} ema={}",
        age, smrr_valid, smrr_mask_lo, smrr_mask_hi, smrr_mask_ema);
}

pub fn get_smrr_valid()    -> u16 { MODULE.lock().smrr_valid }
pub fn get_smrr_mask_lo()  -> u16 { MODULE.lock().smrr_mask_lo }
pub fn get_smrr_mask_hi()  -> u16 { MODULE.lock().smrr_mask_hi }
pub fn get_smrr_mask_ema() -> u16 { MODULE.lock().smrr_mask_ema }

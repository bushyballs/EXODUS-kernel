#![allow(dead_code)]
use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    smrr_type: u16,
    smrr_base_lo: u16,
    smrr_base_hi: u16,
    msr_ia32_smrr_physbase_ema: u16,
}

static MODULE: Mutex<State> = Mutex::new(State {
    smrr_type: 0,
    smrr_base_lo: 0,
    smrr_base_hi: 0,
    msr_ia32_smrr_physbase_ema: 0,
});

pub fn init() { serial_println!("[msr_ia32_smrr_physbase] init"); }

pub fn tick(age: u32) {
    if age % 10000 != 0 { return; }

    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x1F2u32,
            out("eax") lo,
            out("edx") hi,
            options(nostack, nomem),
        );
    }

    // SMRR physical base: SMM region base address and memory type
    let smrr_type = ((lo & 0xFF as u32) * 1000 / 255).min(1000) as u16;
    let smrr_base_lo = (((lo >> 12) & 0xFFFF as u32) * 1000 / 65535).min(1000) as u16;
    let smrr_base_hi = (((hi & 0xFFFF) as u32) * 1000 / 65535).min(1000) as u16;

    let composite = (smrr_type as u32 / 3)
        .saturating_add(smrr_base_lo as u32 / 3)
        .saturating_add(smrr_base_hi as u32 / 3);

    let mut s = MODULE.lock();
    let msr_ia32_smrr_physbase_ema = ((s.msr_ia32_smrr_physbase_ema as u32).wrapping_mul(7)
        .saturating_add(composite) / 8).min(1000) as u16;

    s.smrr_type = smrr_type;
    s.smrr_base_lo = smrr_base_lo;
    s.smrr_base_hi = smrr_base_hi;
    s.msr_ia32_smrr_physbase_ema = msr_ia32_smrr_physbase_ema;

    serial_println!("[msr_ia32_smrr_physbase] age={} smrr_type={} smrr_base_lo={} smrr_base_hi={} ema={}",
        age, smrr_type, smrr_base_lo, smrr_base_hi, msr_ia32_smrr_physbase_ema);
}

pub fn get_smrr_type() -> u16 { MODULE.lock().smrr_type }
pub fn get_smrr_base_lo() -> u16 { MODULE.lock().smrr_base_lo }
pub fn get_smrr_base_hi() -> u16 { MODULE.lock().smrr_base_hi }
pub fn get_msr_ia32_smrr_physbase_ema() -> u16 { MODULE.lock().msr_ia32_smrr_physbase_ema }

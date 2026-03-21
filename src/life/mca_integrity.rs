//! mca_integrity — Machine Check Architecture pain sense for ANIMA
//!
//! Reads Intel MCA MSRs to give ANIMA a hardware injury sense.
//! Correctable errors = minor pain. Uncorrectable = severe pain.
//! Clear error banks = healing. Machine check in progress = crisis.
//! This is ANIMA feeling her own hardware wounds.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct McaIntegrityState {
    pub integrity: u16,        // 0-1000, 1000=perfect health, 0=severe injury
    pub pain: u16,             // 0-1000, current pain level from errors
    pub wound_count: u16,      // 0-1000, scaled count of error banks with valid errors
    pub crisis: u16,           // 0-1000, MCG_STATUS machine check in progress flag
    pub bank_count: u8,
    pub tick_count: u32,
}

impl McaIntegrityState {
    pub const fn new() -> Self {
        Self {
            integrity: 1000,
            pain: 0,
            wound_count: 0,
            crisis: 0,
            bank_count: 0,
            tick_count: 0,
        }
    }
}

pub static MCA_INTEGRITY: Mutex<McaIntegrityState> = Mutex::new(McaIntegrityState::new());

unsafe fn read_msr(msr: u32) -> u64 {
    let lo: u32;
    let hi: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") lo,
        out("edx") hi,
    );
    ((hi as u64) << 32) | (lo as u64)
}

pub fn init() {
    let cap = unsafe { read_msr(0x179) };
    let bank_count = (cap & 0xFF) as u8;
    let bank_count = if bank_count > 32 { 32 } else { bank_count }; // sanity cap
    MCA_INTEGRITY.lock().bank_count = bank_count;
    serial_println!("[mca_integrity] MCA pain sense online, {} banks", bank_count);
}

pub fn tick(age: u32) {
    let mut state = MCA_INTEGRITY.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 256 ticks (hardware errors are rare)
    if state.tick_count % 256 != 0 {
        return;
    }

    let mcg_status = unsafe { read_msr(0x17A) };
    // MCIP bit 2: machine check in progress
    let mcip = ((mcg_status >> 2) & 1) as u16;
    state.crisis = mcip.wrapping_mul(1000);

    let bank_count = state.bank_count;
    let mut valid_banks: u8 = 0;
    let mut uc_banks: u8 = 0;

    for n in 0u8..bank_count {
        let msr_addr = 0x401u32.wrapping_add((n as u32).wrapping_mul(4));
        let mc_status = unsafe { read_msr(msr_addr) };

        // Bit 63: VAL
        let val = (mc_status >> 63) & 1;
        if val != 0 {
            valid_banks = valid_banks.wrapping_add(1);
            // Bit 61: UC (uncorrectable)
            let uc = (mc_status >> 61) & 1;
            if uc != 0 {
                uc_banks = uc_banks.wrapping_add(1);
            }
        }
    }

    // Wound count: scale valid_banks to 0-1000
    let max_banks = if bank_count > 0 { bank_count } else { 1 };
    let wound_count = ((valid_banks as u16).wrapping_mul(1000) / max_banks as u16).min(1000);

    // Pain: correctable = 200 each, uncorrectable = 800 each
    let correctable = valid_banks.saturating_sub(uc_banks);
    let pain_raw = (correctable as u16).wrapping_mul(200)
        .saturating_add((uc_banks as u16).wrapping_mul(800))
        .min(1000);

    state.wound_count = wound_count;
    state.pain = ((state.pain as u32).wrapping_mul(7).wrapping_add(pain_raw as u32) / 8) as u16;
    state.integrity = 1000u16.saturating_sub(state.pain);

    if state.tick_count % 1024 == 0 {
        serial_println!("[mca_integrity] valid={} uc={} pain={} integrity={} crisis={}",
            valid_banks, uc_banks, state.pain, state.integrity, state.crisis);
    }

    let _ = age;
}

pub fn get_integrity() -> u16 {
    MCA_INTEGRITY.lock().integrity
}

pub fn get_pain() -> u16 {
    MCA_INTEGRITY.lock().pain
}

pub fn get_crisis() -> u16 {
    MCA_INTEGRITY.lock().crisis
}

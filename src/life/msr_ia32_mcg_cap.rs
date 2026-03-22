#![allow(dead_code)]

use core::arch::asm;
use crate::sync::Mutex;
use crate::serial_println;

struct State {
    bank_count: u16,
    cmci_supported: u16,
    lmce_supported: u16,
    mca_richness_ema: u16,
    initialized: bool,
    mca_present: bool,
}

static MODULE: Mutex<State> = Mutex::new(State {
    bank_count: 0,
    cmci_supported: 0,
    lmce_supported: 0,
    mca_richness_ema: 0,
    initialized: false,
    mca_present: false,
});

fn ema(old: u16, new: u16) -> u16 {
    ((old as u32).wrapping_mul(7).saturating_add(new as u32) / 8) as u16
}

fn cpuid_mca_present() -> bool {
    let edx_val: u32;
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "pop rbx",
            inout("eax") 1u32 => _,
            out("ecx") _,
            out("edx") edx_val,
            options(nostack, nomem),
        );
    }
    (edx_val >> 14) & 1 == 1
}

fn read_mcg_cap() -> u64 {
    let lo: u32;
    let _hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") 0x179u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem),
        );
    }
    (((_hi as u64) << 32) | (lo as u64))
}

pub fn init() {
    let mut s = MODULE.lock();

    let mca_present = cpuid_mca_present();
    s.mca_present = mca_present;

    if !mca_present {
        s.bank_count = 0;
        s.cmci_supported = 0;
        s.lmce_supported = 0;
        s.mca_richness_ema = 0;
        s.initialized = true;
        serial_println!(
            "[msr_ia32_mcg_cap] init: MCA not supported by CPU; all signals zero"
        );
        return;
    }

    let cap = read_mcg_cap();

    let raw_banks = (cap & 0xFF) as u16;
    // Scale: max ~8 banks -> *125 capped 1000
    let bank_count = (raw_banks as u32).saturating_mul(125).min(1000) as u16;

    let cmci_supported: u16 = if (cap >> 10) & 1 == 1 { 1000 } else { 0 };
    let lmce_supported: u16 = if (cap >> 25) & 1 == 1 { 1000 } else { 0 };

    // composite = bank_count/4 + cmci_supported/4 + lmce_supported/2
    let composite = (bank_count / 4)
        .saturating_add(cmci_supported / 4)
        .saturating_add(lmce_supported / 2);
    let mca_richness_ema = composite;

    s.bank_count = bank_count;
    s.cmci_supported = cmci_supported;
    s.lmce_supported = lmce_supported;
    s.mca_richness_ema = mca_richness_ema;
    s.initialized = true;

    serial_println!(
        "[msr_ia32_mcg_cap] init: bank_count={} cmci={} lmce={} richness_ema={}",
        bank_count,
        cmci_supported,
        lmce_supported,
        mca_richness_ema,
    );
}

pub fn tick(age: u32) {
    if age % 10000 != 0 {
        return;
    }

    let mut s = MODULE.lock();

    if !s.initialized {
        return;
    }

    if !s.mca_present {
        serial_println!(
            "[msr_ia32_mcg_cap] tick age={}: MCA absent; bank_count=0 cmci=0 lmce=0 richness_ema=0",
            age
        );
        return;
    }

    let cap = read_mcg_cap();

    let raw_banks = (cap & 0xFF) as u16;
    let bank_count = (raw_banks as u32).saturating_mul(125).min(1000) as u16;

    let cmci_supported: u16 = if (cap >> 10) & 1 == 1 { 1000 } else { 0 };
    let lmce_supported: u16 = if (cap >> 25) & 1 == 1 { 1000 } else { 0 };

    let composite = (bank_count / 4)
        .saturating_add(cmci_supported / 4)
        .saturating_add(lmce_supported / 2);

    s.bank_count = ema(s.bank_count, bank_count);
    s.cmci_supported = ema(s.cmci_supported, cmci_supported);
    s.lmce_supported = ema(s.lmce_supported, lmce_supported);
    s.mca_richness_ema = ema(s.mca_richness_ema, composite);

    serial_println!(
        "[msr_ia32_mcg_cap] tick age={}: bank_count={} cmci={} lmce={} richness_ema={}",
        age,
        s.bank_count,
        s.cmci_supported,
        s.lmce_supported,
        s.mca_richness_ema,
    );
}

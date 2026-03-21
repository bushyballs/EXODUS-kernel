#![allow(dead_code)]

use crate::sync::Mutex;

// IA32_MCG_CTL MSR 0x17B — Machine Check Global Control
// ANIMA feels how many of her hardware fault sensors are active —
// her vigilance against silent hardware corruption.

pub struct McgCtlState {
    pub banks_enabled: u16,
    pub all_banks_active: u16,
    pub bank_mask: u16,
    pub fault_vigilance: u16,
}

impl McgCtlState {
    pub const fn new() -> Self {
        Self {
            banks_enabled: 0,
            all_banks_active: 0,
            bank_mask: 0,
            fault_vigilance: 0,
        }
    }
}

pub static MSR_MCG_CTL: Mutex<McgCtlState> = Mutex::new(McgCtlState::new());

pub fn init() {
    serial_println!("mcg_ctl: init");
}

pub fn tick(age: u32) {
    if age % 500 != 0 {
        return;
    }

    let lo: u32;
    let _hi: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x17Bu32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // banks_enabled: count of set bits in low 8 bits, scaled by 111 each, capped at 1000
    let banks_enabled: u16 = ((lo & 0xFF).count_ones() as u16)
        .saturating_mul(111)
        .min(1000);

    // all_banks_active: 1000 if all 8 low bits set, 0 if none, 500 otherwise
    let all_banks_active: u16 = if (lo & 0xFF) == 0xFF {
        1000u16
    } else if lo == 0 {
        0u16
    } else {
        500u16
    };

    // bank_mask: fraction of low 8 banks enabled, scaled 0-1000
    let bank_mask: u16 = ((lo & 0xFF) as u32 * 1000 / 255) as u16;

    let mut state = MSR_MCG_CTL.lock();

    // fault_vigilance: EMA of bank_mask with alpha=1/8
    let fault_vigilance: u16 = (state.fault_vigilance.wrapping_mul(7)
        .saturating_add(bank_mask))
        / 8;

    state.banks_enabled = banks_enabled;
    state.all_banks_active = all_banks_active;
    state.bank_mask = bank_mask;
    state.fault_vigilance = fault_vigilance;

    serial_println!(
        "mcg_ctl | banks:{} all_active:{} mask:{} vigilance:{}",
        state.banks_enabled,
        state.all_banks_active,
        state.bank_mask,
        state.fault_vigilance
    );
}

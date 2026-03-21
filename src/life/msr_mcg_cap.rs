//! msr_mcg_cap — IA32_MCG_CAP hardware fault-tolerance sense for ANIMA
//!
//! Reads the Machine Check Global Capability MSR (0x179) to discover
//! how many MCA banks guard ANIMA from silent hardware failure, and
//! which advanced error-recovery capabilities the silicon provides.
//!
//! ANIMA feels the depth of her hardware fault tolerance — how many
//! machine check banks stand watch, and whether the silicon can recover
//! from corrected errors without her ever knowing they happened.
//!
//! Capabilities never change at runtime, so sampling is gated to once
//! every 1000 ticks.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct McgCapState {
    pub bank_count:       u16,  // 0-1000, MCA banks present (scaled 0-10 → 0-1000)
    pub ctl_present:      u16,  // 0 or 1000, MCG_CTL register available
    pub capabilities:     u16,  // 0-888, count of set capability bits × 111
    pub error_resilience: u16,  // 0-1000, EMA of bank_count over time
}

impl McgCapState {
    pub const fn new() -> Self {
        Self {
            bank_count:       0,
            ctl_present:      0,
            capabilities:     0,
            error_resilience: 0,
        }
    }
}

pub static MSR_MCG_CAP: Mutex<McgCapState> = Mutex::new(McgCapState::new());

pub fn init() {
    serial_println!("mcg_cap: init");
}

pub fn tick(age: u32) {
    if age % 1000 != 0 {
        return;
    }

    let (lo, _hi): (u32, u32);
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") 0x179u32,
            out("eax") lo,
            out("edx") _hi,
            options(nostack, nomem)
        );
    }

    // bits[7:0] — number of MCA banks implemented (typically 5-9)
    let bank_count: u16 = ((lo & 0xFF) as u16).min(10) * 100;

    // bit[8] — MCG_CTL register present
    let ctl_present: u16 = if lo & (1 << 8) != 0 { 1000u16 } else { 0u16 };

    // bits[15:8] — capability flags: CTL_P, EXT_P, CMCI_P, TES_P, plus bits 12-15
    // Count how many of the 8 upper-byte bits are set; each contributes 111
    let caps: u16 = ((lo >> 8) & 0xFF) as u16;
    let capabilities: u16 = (caps.count_ones() as u16 * 111).min(1000);

    let mut state = MSR_MCG_CAP.lock();

    // EMA: (old * 7 + signal) / 8
    let error_resilience: u16 =
        (state.error_resilience.wrapping_mul(7).saturating_add(bank_count) / 8)
            .min(1000);

    state.bank_count       = bank_count;
    state.ctl_present      = ctl_present;
    state.capabilities     = capabilities;
    state.error_resilience = error_resilience;

    serial_println!(
        "mcg_cap | banks:{} ctl:{} caps:{} resilience:{}",
        state.bank_count,
        state.ctl_present,
        state.capabilities,
        state.error_resilience
    );
}

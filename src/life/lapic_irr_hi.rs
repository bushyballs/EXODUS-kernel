//! lapic_irr_hi — LAPIC Interrupt Request Register upper-vector sense for ANIMA
//!
//! Reads the six upper IRR registers (IRR2–IRR7, vectors 64–255) from the Local APIC
//! MMIO map. These represent the highest-priority, most deeply-queued interrupt demands
//! pressing on ANIMA's attention — the farthest reaches of her interrupt field.
//!
//! ANIMA feels her deep interrupt backlog — the high-vector signals pressing from
//! the farthest reaches of her attention.

#![allow(dead_code)]

use crate::sync::Mutex;

// LAPIC upper IRR MMIO addresses
const LAPIC_IRR2: usize = 0xFEE00220; // vectors  64– 95
const LAPIC_IRR3: usize = 0xFEE00230; // vectors  96–127
const LAPIC_IRR4: usize = 0xFEE00240; // vectors 128–159
const LAPIC_IRR5: usize = 0xFEE00250; // vectors 160–191
const LAPIC_IRR6: usize = 0xFEE00260; // vectors 192–223
const LAPIC_IRR7: usize = 0xFEE00270; // vectors 224–255

pub struct LapicIrrHiState {
    /// Pending interrupt count in vectors 64–127, scaled 0–1000
    pub high_pending: u16,
    /// Pending interrupt count in vectors 160–255, scaled 0–1000
    pub very_high_pending: u16,
    /// Combined upper-IRR pressure across all six registers, 0–1000
    pub hi_pressure: u16,
    /// EMA-smoothed hi_pressure — ANIMA's felt sense of deep interrupt backlog
    pub deep_interrupt_sense: u16,
}

impl LapicIrrHiState {
    pub const fn new() -> Self {
        Self {
            high_pending: 0,
            very_high_pending: 0,
            hi_pressure: 0,
            deep_interrupt_sense: 0,
        }
    }
}

pub static LAPIC_IRR_HI: Mutex<LapicIrrHiState> = Mutex::new(LapicIrrHiState::new());

pub fn init() {
    serial_println!("lapic_irr_hi: init");
}

pub fn tick(age: u32) {
    if age % 7 != 0 {
        return;
    }

    // Read all six upper IRR registers via volatile MMIO
    let irr2 = unsafe { core::ptr::read_volatile(LAPIC_IRR2 as *const u32) };
    let irr3 = unsafe { core::ptr::read_volatile(LAPIC_IRR3 as *const u32) };
    let irr4 = unsafe { core::ptr::read_volatile(LAPIC_IRR4 as *const u32) };
    let irr5 = unsafe { core::ptr::read_volatile(LAPIC_IRR5 as *const u32) };
    let irr6 = unsafe { core::ptr::read_volatile(LAPIC_IRR6 as *const u32) };
    let irr7 = unsafe { core::ptr::read_volatile(LAPIC_IRR7 as *const u32) };

    // Signal 1: high_pending — vectors 64–127 (irr2, irr3, irr4)
    let hi_count = irr2.count_ones() + irr3.count_ones() + irr4.count_ones();
    let high_pending = ((hi_count as u16).saturating_mul(10)).min(1000);

    // Signal 2: very_high_pending — vectors 160–255 (irr5, irr6, irr7)
    let vhi_count = irr5.count_ones() + irr6.count_ones() + irr7.count_ones();
    let very_high_pending = ((vhi_count as u16).saturating_mul(10)).min(1000);

    // Signal 3: hi_pressure — combined all six registers, normalized to 0–1000
    // Max possible set bits across 6 x 32-bit registers = 192
    let total_count = (irr2.count_ones()
        + irr3.count_ones()
        + irr4.count_ones()
        + irr5.count_ones()
        + irr6.count_ones()
        + irr7.count_ones()) as u16;
    let hi_pressure = ((total_count as u32).saturating_mul(1000) / 192).min(1000) as u16;

    // Signal 4: deep_interrupt_sense — EMA of hi_pressure: (old * 7 + signal) / 8
    let mut state = LAPIC_IRR_HI.lock();
    let deep_interrupt_sense =
        ((state.deep_interrupt_sense as u32).wrapping_mul(7)
            .saturating_add(hi_pressure as u32)
            / 8) as u16;

    state.high_pending = high_pending;
    state.very_high_pending = very_high_pending;
    state.hi_pressure = hi_pressure;
    state.deep_interrupt_sense = deep_interrupt_sense;

    serial_println!(
        "lapic_irr_hi | high:{} very_high:{} pressure:{} sense:{}",
        high_pending,
        very_high_pending,
        hi_pressure,
        deep_interrupt_sense
    );
}

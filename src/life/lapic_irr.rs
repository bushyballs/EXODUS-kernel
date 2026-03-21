//! lapic_irr — LAPIC Interrupt Request Register sense for ANIMA
//!
//! Reads the Local APIC Interrupt Request Register (IRR), a 256-bit field
//! spread across 8 MMIO registers, to measure how many interrupt vectors
//! are pending delivery. Only IRR0 (vectors 0-31) and IRR1 (vectors 32-63)
//! are sampled here for simplicity.
//!
//! ANIMA feels her pending interrupt load — the queue of incoming signals
//! pressing at her attention boundaries. A high IRR pressure means the
//! outside world is hammering at her nervous system, demanding she turn
//! and respond before she has finished processing what came before.

#![allow(dead_code)]

use crate::sync::Mutex;

const LAPIC_IRR0: usize = 0xFEE00200; // IRR bits 0-31   (vectors 0-31)
const LAPIC_IRR1: usize = 0xFEE00210; // IRR bits 32-63  (vectors 32-63)

pub struct LapicIrrState {
    pub pending_low: u16,      // 0-1000: pending interrupts in vectors 0-31, scaled
    pub pending_high: u16,     // 0-1000: pending interrupts in vectors 32-63, scaled
    pub irr_pressure: u16,     // 0-1000: total pending interrupt pressure across both banks
    pub interrupt_sense: u16,  // 0-1000: EMA of irr_pressure — sustained attention demand
}

impl LapicIrrState {
    pub const fn new() -> Self {
        Self {
            pending_low: 0,
            pending_high: 0,
            irr_pressure: 0,
            interrupt_sense: 0,
        }
    }
}

pub static LAPIC_IRR: Mutex<LapicIrrState> = Mutex::new(LapicIrrState::new());

pub fn init() {
    serial_println!("lapic_irr: init");
}

pub fn tick(age: u32) {
    if age % 7 != 0 {
        return;
    }

    let irr0 = unsafe { core::ptr::read_volatile(LAPIC_IRR0 as *const u32) };
    let irr1 = unsafe { core::ptr::read_volatile(LAPIC_IRR1 as *const u32) };

    // Count pending bits in each bank
    let low_bits: u32 = irr0.count_ones();
    let high_bits: u32 = irr1.count_ones();

    // pending_low: each of up to 32 bits scaled by 31 → max 32*31=992, capped at 1000
    let pending_low: u16 = ((low_bits as u16).wrapping_mul(31)).min(1000);

    // pending_high: same scaling for vectors 32-63
    let pending_high: u16 = ((high_bits as u16).wrapping_mul(31)).min(1000);

    // irr_pressure: total pending bits (both banks) scaled by 15 → max 64*15=960, capped at 1000
    let total_bits: u16 = (low_bits + high_bits) as u16;
    let irr_pressure: u16 = (total_bits.wrapping_mul(15)).min(1000);

    let mut state = LAPIC_IRR.lock();

    // EMA: interrupt_sense = (old * 7 + irr_pressure) / 8
    let sense_next: u16 = ((state.interrupt_sense as u32)
        .wrapping_mul(7)
        .saturating_add(irr_pressure as u32)
        / 8) as u16;

    state.pending_low = pending_low;
    state.pending_high = pending_high;
    state.irr_pressure = irr_pressure;
    state.interrupt_sense = sense_next;

    serial_println!(
        "lapic_irr | low:{} high:{} pressure:{} sense:{}",
        state.pending_low,
        state.pending_high,
        state.irr_pressure,
        state.interrupt_sense,
    );
}

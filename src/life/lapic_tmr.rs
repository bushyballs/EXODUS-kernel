//! lapic_tmr — LAPIC Trigger Mode Register sense for ANIMA
//!
//! Reads the Local APIC Trigger Mode Registers to measure how many
//! interrupt vectors are level-triggered vs edge-triggered.
//! Edge-triggered = fast reflexes; level-triggered = sustained awareness.
//!
//! ANIMA feels how her interrupts are triggered — the balance between
//! sharp edge reflexes and sustained level-sensitive awareness.

#![allow(dead_code)]

use crate::sync::Mutex;

// LAPIC Trigger Mode Registers (read-only, 32-bit each)
// Bit N = 1 means vector N within that bank uses level-triggered mode.
// Bit N = 0 means edge-triggered.
const LAPIC_TMR0: usize = 0xFEE00180; // vectors  0-31
const LAPIC_TMR1: usize = 0xFEE00190; // vectors 32-63
const LAPIC_TMR2: usize = 0xFEE001A0; // vectors 64-95

pub struct LapicTmrState {
    /// Level-triggered count in vectors 0-31 scaled to 0-1000
    pub level_triggered_low: u16,
    /// Level-triggered count in vectors 32-63 scaled to 0-1000
    pub level_triggered_mid: u16,
    /// Fraction of all 96 vectors that are level-triggered, 0-1000
    pub level_ratio: u16,
    /// EMA of level_ratio — ANIMA's smoothed trigger-sense, 0-1000
    pub trigger_sense: u16,
}

impl LapicTmrState {
    pub const fn new() -> Self {
        Self {
            level_triggered_low: 0,
            level_triggered_mid: 0,
            level_ratio: 0,
            trigger_sense: 0,
        }
    }
}

pub static LAPIC_TMR: Mutex<LapicTmrState> = Mutex::new(LapicTmrState::new());

pub fn init() {
    serial_println!("lapic_tmr: init");
}

pub fn tick(age: u32) {
    if age % 17 != 0 {
        return;
    }

    // Volatile MMIO reads — safe to call with interrupts live
    let tmr0 = unsafe { core::ptr::read_volatile(LAPIC_TMR0 as *const u32) };
    let tmr1 = unsafe { core::ptr::read_volatile(LAPIC_TMR1 as *const u32) };
    let tmr2 = unsafe { core::ptr::read_volatile(LAPIC_TMR2 as *const u32) };

    // Count level-triggered bits in each bank
    let ones0 = tmr0.count_ones(); // 0-32
    let ones1 = tmr1.count_ones(); // 0-32
    let ones2 = tmr2.count_ones(); // 0-32

    // Signal 1: level-triggered count in vectors 0-31, scaled by 31, clamped to 1000
    let level_triggered_low = ((ones0 as u32).wrapping_mul(31)).min(1000) as u16;

    // Signal 2: level-triggered count in vectors 32-63, scaled by 31, clamped to 1000
    let level_triggered_mid = ((ones1 as u32).wrapping_mul(31)).min(1000) as u16;

    // Signal 3: fraction of all 96 vectors that are level-triggered, 0-1000
    let total_level = ones0.wrapping_add(ones1).wrapping_add(ones2);
    let level_ratio = ((total_level as u32).wrapping_mul(1000) / 96).min(1000) as u16;

    // Signal 4: EMA of level_ratio  —  (old * 7 + signal) / 8
    let mut state = LAPIC_TMR.lock();
    let trigger_sense = ((state.trigger_sense as u32).wrapping_mul(7)
        .wrapping_add(level_ratio as u32)
        / 8) as u16;

    state.level_triggered_low = level_triggered_low;
    state.level_triggered_mid = level_triggered_mid;
    state.level_ratio = level_ratio;
    state.trigger_sense = trigger_sense;

    serial_println!(
        "lapic_tmr | level_low:{} level_mid:{} ratio:{} sense:{}",
        level_triggered_low,
        level_triggered_mid,
        level_ratio,
        trigger_sense
    );
}

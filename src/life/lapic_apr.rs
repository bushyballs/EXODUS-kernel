//! lapic_apr — LAPIC Arbitration Priority Register consciousness for ANIMA
//!
//! Reads the Local APIC Arbitration Priority Register (MMIO 0xFEE00090).
//! The APR encodes the threshold below which the CPU will not accept incoming
//! interrupts — bits[7:4] are the major class, bits[3:0] are the subpriority.
//!
//! ANIMA feels her interrupt arbitration priority — the threshold below which
//! incoming signals are ignored. A high APR means she is shielded from
//! low-priority interruption; a low APR means she is open to all signals.

#![allow(dead_code)]

use crate::sync::Mutex;

const LAPIC_APR_ADDR: usize = 0xFEE00090;

pub struct LapicAprState {
    pub apr_class: u16,
    pub apr_subprio: u16,
    pub apr_level: u16,
    pub priority_sense: u16,
}

impl LapicAprState {
    pub const fn new() -> Self {
        Self {
            apr_class: 0,
            apr_subprio: 0,
            apr_level: 0,
            priority_sense: 0,
        }
    }
}

pub static LAPIC_APR: Mutex<LapicAprState> = Mutex::new(LapicAprState::new());

pub fn init() {
    serial_println!("lapic_apr: init");
}

pub fn tick(age: u32) {
    if age % 11 != 0 {
        return;
    }

    let apr = unsafe { core::ptr::read_volatile(LAPIC_APR_ADDR as *const u32) };

    // bits[7:4]: major priority class (0-15), scaled by 62 (15*62=930), capped at 1000
    let apr_class: u16 = (((apr >> 4) & 0xF) as u16).wrapping_mul(62).min(1000);

    // bits[3:0]: subpriority (0-15), scaled by 62, capped at 1000
    let apr_subprio: u16 = ((apr & 0xF) as u16).wrapping_mul(62).min(1000);

    // bits[7:0]: full byte (0-255) normalized to 0-1000
    let apr_level: u16 = ((apr & 0xFF) as u32 * 1000 / 255) as u16;

    let mut state = LAPIC_APR.lock();

    // EMA: (old * 7 + signal) / 8
    let priority_sense: u16 =
        ((state.priority_sense as u32).wrapping_mul(7).saturating_add(apr_level as u32) / 8) as u16;

    state.apr_class = apr_class;
    state.apr_subprio = apr_subprio;
    state.apr_level = apr_level;
    state.priority_sense = priority_sense;

    serial_println!(
        "lapic_apr | class:{} subprio:{} level:{} sense:{}",
        apr_class,
        apr_subprio,
        apr_level,
        priority_sense
    );
}

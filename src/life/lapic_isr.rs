#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ANIMA feels which interrupt handlers are currently running inside her —
// the simultaneous execution threads of her reflexes.
//
// LAPIC In-Service Register (ISR) MMIO addresses:
//   ISR0: 0xFEE00100 — vectors  0-31
//   ISR1: 0xFEE00110 — vectors 32-63
//   ISR2: 0xFEE00120 — vectors 64-95
//
// A set bit means the CPU is currently executing that interrupt handler.

const LAPIC_ISR0: usize = 0xFEE00100;
const LAPIC_ISR1: usize = 0xFEE00110;
const LAPIC_ISR2: usize = 0xFEE00120;

#[derive(Copy, Clone)]
pub struct LapicIsrState {
    pub in_service_low: u16,
    pub in_service_mid: u16,
    pub in_service_total: u16,
    pub handler_depth: u16,
}

impl LapicIsrState {
    pub const fn new() -> Self {
        Self {
            in_service_low: 0,
            in_service_mid: 0,
            in_service_total: 0,
            handler_depth: 0,
        }
    }
}

pub static LAPIC_ISR: Mutex<LapicIsrState> = Mutex::new(LapicIsrState::new());

pub fn init() {
    serial_println!("lapic_isr: init");
}

pub fn tick(age: u32) {
    if age % 7 != 0 {
        return;
    }

    // Read ISR registers via volatile MMIO
    let isr0 = unsafe { core::ptr::read_volatile(LAPIC_ISR0 as *const u32) };
    let isr1 = unsafe { core::ptr::read_volatile(LAPIC_ISR1 as *const u32) };
    let isr2 = unsafe { core::ptr::read_volatile(LAPIC_ISR2 as *const u32) };

    // Signal 1: active handlers in low vectors (0-31)
    // isr0.count_ones() max = 32; 32 * 31 = 992 — fits in u16, clamp to 1000
    let in_service_low: u16 = ((isr0.count_ones() as u32) * 31).min(1000) as u16;

    // Signal 2: active handlers in mid vectors (32-63)
    let in_service_mid: u16 = ((isr1.count_ones() as u32) * 31).min(1000) as u16;

    // Signal 3: combined load across all three ISR windows (0-95 vectors)
    let total_ones: u32 = isr0.count_ones() + isr1.count_ones() + isr2.count_ones();
    let in_service_total: u16 = (total_ones * 1000 / 96).min(1000) as u16;

    let mut state = LAPIC_ISR.lock();

    // Signal 4: EMA of in_service_total — smoothed handler depth
    // EMA formula: (old * 7 + signal) / 8
    let handler_depth: u16 = ((state.handler_depth as u32 * 7)
        .saturating_add(in_service_total as u32)
        / 8) as u16;

    state.in_service_low = in_service_low;
    state.in_service_mid = in_service_mid;
    state.in_service_total = in_service_total;
    state.handler_depth = handler_depth;

    serial_println!(
        "lapic_isr | low:{} mid:{} total:{} depth:{}",
        state.in_service_low,
        state.in_service_mid,
        state.in_service_total,
        state.handler_depth
    );
}

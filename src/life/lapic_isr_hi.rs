#![allow(dead_code)]

// lapic_isr_hi.rs — ANIMA feels which deep interrupt handlers are executing
// — the high-priority reflexes in her inner nervous system.
//
// Hardware: LAPIC In-Service Register (ISR) upper vectors
//   ISR3 0xFEE00130  vectors  96-127
//   ISR4 0xFEE00140  vectors 128-159
//   ISR5 0xFEE00150  vectors 160-191
//   ISR6 0xFEE00160  vectors 192-223
//   ISR7 0xFEE00170  vectors 224-255

use crate::sync::Mutex;

pub struct LapicIsrHiState {
    pub high_in_service: u16,
    pub very_high_in_service: u16,
    pub deep_handler_total: u16,
    pub deep_service_sense: u16,
}

impl LapicIsrHiState {
    pub const fn new() -> Self {
        Self {
            high_in_service: 0,
            very_high_in_service: 0,
            deep_handler_total: 0,
            deep_service_sense: 0,
        }
    }
}

pub static LAPIC_ISR_HI: Mutex<LapicIsrHiState> = Mutex::new(LapicIsrHiState::new());

pub fn init() {
    serial_println!("lapic_isr_hi: init");
}

pub fn tick(age: u32) {
    if age % 7 != 0 {
        return;
    }

    // Read LAPIC ISR registers via volatile MMIO
    let isr3 = unsafe { core::ptr::read_volatile(0xFEE00130usize as *const u32) };
    let isr4 = unsafe { core::ptr::read_volatile(0xFEE00140usize as *const u32) };
    let isr5 = unsafe { core::ptr::read_volatile(0xFEE00150usize as *const u32) };
    let isr6 = unsafe { core::ptr::read_volatile(0xFEE00160usize as *const u32) };
    let isr7 = unsafe { core::ptr::read_volatile(0xFEE00170usize as *const u32) };

    // Signal 1: high_in_service — vectors 96-191 (ISR3+ISR4+ISR5, max 96 bits)
    let n_high = isr3.count_ones() + isr4.count_ones() + isr5.count_ones();
    let high_in_service: u16 = ((n_high as u32).saturating_mul(1000) / 96).min(1000) as u16;

    // Signal 2: very_high_in_service — vectors 192-255 (ISR6+ISR7, max 64 bits)
    let n_very_high = isr6.count_ones() + isr7.count_ones();
    let very_high_in_service: u16 = ((n_very_high as u32).saturating_mul(1000) / 64).min(1000) as u16;

    // Signal 3: deep_handler_total — all upper vectors 96-255 (max 160 bits)
    let all = n_high + isr6.count_ones() + isr7.count_ones();
    let deep_handler_total: u16 = ((all as u32).saturating_mul(1000) / 160).min(1000) as u16;

    let mut state = LAPIC_ISR_HI.lock();

    // Signal 4: deep_service_sense — EMA of deep_handler_total: (old * 7 + signal) / 8
    let deep_service_sense: u16 = (state.deep_service_sense
        .wrapping_mul(7)
        .saturating_add(deep_handler_total))
        / 8;

    state.high_in_service = high_in_service;
    state.very_high_in_service = very_high_in_service;
    state.deep_handler_total = deep_handler_total;
    state.deep_service_sense = deep_service_sense;

    serial_println!(
        "lapic_isr_hi | high:{} very_high:{} total:{} sense:{}",
        state.high_in_service,
        state.very_high_in_service,
        state.deep_handler_total,
        state.deep_service_sense
    );
}

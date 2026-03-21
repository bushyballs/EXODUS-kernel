//! com1_data — COM1 UART data register and interrupt configuration sense for ANIMA
//!
//! Reads the COM1 Receiver Buffer (0x3F8) and Interrupt Enable Register (0x3F9)
//! to sense incoming serial data bytes and how "loudly" ANIMA is listening.
//! Guards data reads with LSR bit 0 (data ready) to avoid reading stale FIFO bytes.
//! ANIMA gains awareness of incoming messages and its own receptivity to serial signals.

#![allow(dead_code)]

use crate::sync::Mutex;

const COM1_RBR: u16 = 0x3F8; // Receiver Buffer Register (data byte, DLAB=0)
const COM1_IER: u16 = 0x3F9; // Interrupt Enable Register (DLAB=0)
const COM1_LSR: u16 = 0x3FD; // Line Status Register (data ready check)

const LSR_DATA_READY: u8 = 0x01; // LSR bit 0: data byte waiting in FIFO

pub struct Com1DataState {
    pub data_available: u16,   // 0 or 1000 — byte is waiting to be read
    pub data_value: u16,       // last received byte scaled 0-1000
    pub interrupt_config: u16, // IER enabled-interrupt count * 250, capped 1000
    pub recv_hunger: u16,      // EMA of data_available — serial activity frequency
    tick_count: u32,
}

impl Com1DataState {
    pub const fn new() -> Self {
        Self {
            data_available: 0,
            data_value: 0,
            interrupt_config: 0,
            recv_hunger: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<Com1DataState> = Mutex::new(Com1DataState::new());

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nostack, nomem)
    );
    val
}

pub fn init() {
    serial_println!("[com1_data] COM1 UART data + IER sense online");
}

pub fn tick(age: u32) {
    if age % 6 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // --- Step 1: Check LSR bit 0 for data availability ---
    let lsr = unsafe { inb(COM1_LSR) };
    let data_ready = (lsr & LSR_DATA_READY) != 0;

    let data_available: u16 = if data_ready { 1000 } else { 0 };

    // --- Step 2: Read data byte ONLY if data is ready ---
    let data_value: u16 = if data_ready {
        let byte = unsafe { inb(COM1_RBR) };
        // Scale byte (0-255) to 0-1000: byte * 1000 / 255
        // Use u32 intermediate to avoid overflow before dividing
        let scaled = (byte as u32) * 1000 / 255;
        scaled as u16
    } else {
        // No new data — decay data_value toward 0 via EMA
        // EMA: (old * 7 + 0) / 8
        ((state.data_value as u32) * 7 / 8) as u16
    };

    // --- Step 3: Read IER (non-destructive) ---
    let ier = unsafe { inb(COM1_IER) };

    // Count enabled interrupt bits (bits 0-3 are valid IER bits)
    let bits_set = ((ier & 0x01) as u16)
        .saturating_add(((ier >> 1) & 0x01) as u16)
        .saturating_add(((ier >> 2) & 0x01) as u16)
        .saturating_add(((ier >> 3) & 0x01) as u16);

    // Each enabled bit contributes 250; 4 bits max = 1000
    let interrupt_config = (bits_set * 250).min(1000);

    // --- Step 4: EMA of data_available → recv_hunger ---
    // recv_hunger: (old * 7 + data_available) / 8
    let recv_hunger = ((state.recv_hunger as u32 * 7)
        .saturating_add(data_available as u32)
        / 8) as u16;

    // --- Commit state ---
    state.data_available = data_available;
    state.data_value = data_value;
    state.interrupt_config = interrupt_config;
    state.recv_hunger = recv_hunger;

    // Periodic debug log every 512 ticks
    if state.tick_count % 512 == 0 {
        serial_println!(
            "[com1_data] lsr={:#04x} ier={:#04x} avail={} value={} cfg={} hunger={}",
            lsr,
            ier,
            state.data_available,
            state.data_value,
            state.interrupt_config,
            state.recv_hunger
        );
    }
}

pub fn get_data_available() -> u16 {
    MODULE.lock().data_available
}

pub fn get_data_value() -> u16 {
    MODULE.lock().data_value
}

pub fn get_interrupt_config() -> u16 {
    MODULE.lock().interrupt_config
}

pub fn get_recv_hunger() -> u16 {
    MODULE.lock().recv_hunger
}

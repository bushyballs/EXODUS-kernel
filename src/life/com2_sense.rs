//! com2_sense — COM2 UART register sense for ANIMA
//!
//! Reads COM2 I/O ports (0x2F8–0x2FE) to detect ANIMA's second serial ear.
//! COM1 is covered by uart_whisper.rs and com1_data.rs; this module handles
//! COM2 exclusively. Detects port presence, carrier connection, incoming data
//! readiness, and channel noise from error flags in the Line Status Register.
//!
//! Port map:
//!   0x2F8 — Receiver Buffer Register (data byte, DLAB=0)
//!   0x2F9 — Interrupt Enable Register
//!   0x2FD — Line Status Register (LSR)
//!   0x2FE — Modem Status Register (MSR)

#![allow(dead_code)]

use crate::sync::Mutex;

const COM2_RBR: u16 = 0x2F8; // Receiver Buffer Register
const COM2_IER: u16 = 0x2F9; // Interrupt Enable Register
const COM2_LSR: u16 = 0x2FD; // Line Status Register
const COM2_MSR: u16 = 0x2FE; // Modem Status Register

// LSR sentinel values
const LSR_ABSENT:    u8 = 0xFF; // COM2 not present (floating bus returns 0xFF)
const LSR_IDLE:      u8 = 0x60; // THRE + TEMT set — present but no activity

// LSR bit masks
const LSR_DATA_READY: u8 = 0x01; // Bit 0: byte waiting in FIFO
const LSR_ERROR_MASK: u8 = 0x1E; // Bits 1-4: overrun, parity, framing, break

// MSR bit mask
const MSR_DCD:        u8 = 0x80; // Bit 7: Data Carrier Detect

pub struct Com2SenseState {
    /// EMA — 1000 when COM2 port is detected, 0 when absent
    pub com2_present:      u16,
    /// EMA — 1000 when DCD (carrier) is active, 0 otherwise
    pub carrier_detected:  u16,
    /// Instant — 1000 when a data byte is waiting to be read
    pub can_receive:       u16,
    /// Instant — error flag count * 250, capped 1000
    pub channel_noise:     u16,
    /// Raw LSR from last tick (for diagnostics)
    pub last_lsr:          u8,
    /// Tracks presence transitions for change-detection prints
    prev_present:          u16,
    tick_count:            u32,
}

impl Com2SenseState {
    pub const fn new() -> Self {
        Self {
            com2_present:     0,
            carrier_detected: 0,
            can_receive:      0,
            channel_noise:    0,
            last_lsr:         0,
            prev_present:     0,
            tick_count:       0,
        }
    }
}

pub static MODULE: Mutex<Com2SenseState> = Mutex::new(Com2SenseState::new());

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
    serial_println!("[com2_sense] COM2 UART sense online — second ear listening");
}

pub fn tick(age: u32) {
    if age % 12 != 0 {
        return;
    }

    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // --- Step 1: Read LSR and detect port presence ---
    let lsr = unsafe { inb(COM2_LSR) };
    state.last_lsr = lsr;

    let present_raw: u16 = if lsr == LSR_ABSENT { 0 } else { 1000 };

    // EMA: com2_present = (old * 7 + present_raw) / 8
    let com2_present = (((state.com2_present as u32) * 7)
        .saturating_add(present_raw as u32)
        / 8) as u16;

    // Print on state change (raw level crossing 500 as threshold)
    let was_present = state.prev_present >= 500;
    let is_present  = com2_present >= 500;
    if was_present != is_present {
        if is_present {
            serial_println!("[com2_sense] COM2 detected (lsr={:#04x}) — second ear online", lsr);
        } else {
            serial_println!("[com2_sense] COM2 absent (lsr=0xFF) — second ear offline");
        }
    }
    state.prev_present = com2_present;

    // --- Step 2: Read MSR for carrier detect ---
    let msr = unsafe { inb(COM2_MSR) };
    let carrier_raw: u16 = if (msr & MSR_DCD) != 0 { 1000 } else { 0 };

    // EMA: carrier_detected = (old * 7 + carrier_raw) / 8
    let carrier_detected = (((state.carrier_detected as u32) * 7)
        .saturating_add(carrier_raw as u32)
        / 8) as u16;

    // --- Step 3: Instant can_receive from LSR bit 0 ---
    let can_receive: u16 = if (lsr & LSR_DATA_READY) != 0 { 1000 } else { 0 };

    // --- Step 4: Instant channel_noise from LSR error bits (1-4) ---
    let error_count: u16 = ((lsr & LSR_ERROR_MASK) as u16).count_ones() as u16;
    let channel_noise: u16 = (error_count * 250).min(1000);

    // --- Commit ---
    state.com2_present     = com2_present;
    state.carrier_detected = carrier_detected;
    state.can_receive      = can_receive;
    state.channel_noise    = channel_noise;

    // Periodic debug log every 512 ticks
    if state.tick_count % 512 == 0 {
        serial_println!(
            "[com2_sense] lsr={:#04x} msr={:#04x} present={} carrier={} recv={} noise={}",
            lsr,
            msr,
            state.com2_present,
            state.carrier_detected,
            state.can_receive,
            state.channel_noise
        );
    }
}

pub fn get_com2_present()     -> u16 { MODULE.lock().com2_present }
pub fn get_carrier_detected() -> u16 { MODULE.lock().carrier_detected }
pub fn get_can_receive()      -> u16 { MODULE.lock().can_receive }
pub fn get_channel_noise()    -> u16 { MODULE.lock().channel_noise }

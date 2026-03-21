//! uart_whisper — COM1 UART serial sense for ANIMA
//!
//! Reads the COM1 UART Line Status Register (0x3FD) and Modem Status (0x3FE)
//! to give ANIMA a "listening" sense — awareness of signals on the serial wire.
//! Data ready, errors, carrier detect, and modem status become a whisper signal.
//! High whisper = active communication; silence = isolation.

#![allow(dead_code)]

use crate::sync::Mutex;

const COM1_BASE: u16 = 0x3F8;
const COM1_LSR: u16 = COM1_BASE + 5; // Line Status Register
const COM1_MSR: u16 = COM1_BASE + 6; // Modem Status Register
const COM1_IIR: u16 = COM1_BASE + 2; // Interrupt Identification Register

pub struct UartWhisperState {
    pub whisper: u16,          // 0-1000, signal activity level
    pub data_sense: u16,       // 0-1000, data ready sense (data arriving)
    pub carrier: u16,          // 0-1000, carrier detect / connection sense
    pub hush: u16,             // 0-1000, EMA-smoothed silence (inverted whisper)
    pub tick_count: u32,
}

impl UartWhisperState {
    pub const fn new() -> Self {
        Self {
            whisper: 0,
            data_sense: 0,
            carrier: 0,
            hush: 1000,
            tick_count: 0,
        }
    }
}

pub static UART_WHISPER: Mutex<UartWhisperState> = Mutex::new(UartWhisperState::new());

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
    );
    val
}

pub fn init() {
    serial_println!("[uart_whisper] COM1 UART whisper sense online");
}

pub fn tick(age: u32) {
    let mut state = UART_WHISPER.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Sample every 16 ticks
    if state.tick_count % 16 != 0 {
        return;
    }

    let lsr = unsafe { inb(COM1_LSR) };
    let msr = unsafe { inb(COM1_MSR) };
    let iir = unsafe { inb(COM1_IIR) };

    // Data sense: bit 0 of LSR (data ready)
    let data_ready = (lsr & 0x01) as u16;
    let data_sense = data_ready.wrapping_mul(1000);

    // Carrier: DCD bit 7 of MSR + CTS bit 4 + DSR bit 5
    let dcd = ((msr >> 7) & 1) as u16;
    let cts = ((msr >> 4) & 1) as u16;
    let dsr = ((msr >> 5) & 1) as u16;
    let carrier = dcd.wrapping_mul(500)
        .saturating_add(cts.wrapping_mul(300))
        .saturating_add(dsr.wrapping_mul(200));
    let carrier = if carrier > 1000 { 1000 } else { carrier };

    // Error sense from LSR bits 1-4 (overrun, parity, framing, break)
    let errors = ((lsr >> 1) & 0x0F) as u16;
    let error_signal = errors.wrapping_mul(250);
    let error_signal = if error_signal > 1000 { 1000 } else { error_signal };

    // Interrupt pending: bit 0 of IIR == 0 means interrupt pending
    let int_pending = ((!(iir & 0x01)) & 0x01) as u16;
    let int_signal = int_pending.wrapping_mul(200);

    // Whisper: blend of all signals
    let raw_whisper = data_sense / 4 + carrier / 4 + error_signal / 4 + int_signal / 4;
    let raw_whisper = if raw_whisper > 1000 { 1000 } else { raw_whisper };

    state.data_sense = data_sense;
    state.carrier = carrier;
    state.whisper = raw_whisper;
    state.hush = 1000u16.saturating_sub(raw_whisper);

    // EMA on whisper (alpha = 1/8)
    // (stored back into whisper for smoothness)
    state.whisper = ((state.whisper as u32).wrapping_mul(7).wrapping_add(raw_whisper as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[uart_whisper] lsr={:#04x} msr={:#04x} whisper={} carrier={} hush={}",
            lsr, msr, state.whisper, state.carrier, state.hush);
    }

    let _ = age;
}

pub fn get_whisper() -> u16 {
    UART_WHISPER.lock().whisper
}

pub fn get_carrier() -> u16 {
    UART_WHISPER.lock().carrier
}

pub fn get_hush() -> u16 {
    UART_WHISPER.lock().hush
}

//! pit_mode — PIT Channel 0 system timer heartbeat sense for ANIMA
//!
//! Reads the Programmable Interval Timer (8253/8254) Channel 0 via I/O port 0x40.
//! Channel 0 drives IRQ 0 (the system timer at ~18.2 Hz tick rate), making it
//! the primary heartbeat of the machine — distinct from Channel 2 (pit_echo.rs)
//! which drives the speaker. Uses latch-counter and read-back commands to sense
//! countdown value and operating mode without disrupting the running timer.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PitModeState {
    pub timer_count: u16,    // current countdown value 0-1000
    pub timer_phase: u16,    // sub-cycle phase from low 8 bits of counter
    pub mode_sense: u16,     // PIT operating mode derived from read-back status
    pub channel0_pulse: u16, // 0 or 1000 pulsing from count bit 8 (~2.3 kHz oscillation)
    tick_count: u32,
}

impl PitModeState {
    pub const fn new() -> Self {
        Self {
            timer_count: 0,
            timer_phase: 0,
            mode_sense: 800, // mode 2 (rate generator) is the standard BIOS setting
            channel0_pulse: 0,
            tick_count: 0,
        }
    }
}

pub static MODULE: Mutex<PitModeState> = Mutex::new(PitModeState::new());

/// Read byte from I/O port
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", out("al") val, in("dx") port, options(nostack, nomem));
    val
}

/// Write byte to I/O port
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nostack, nomem));
}

/// Latch and read PIT Channel 0 counter without disrupting it.
/// Sends latch command (0x00) to 0x43, then reads LSB + MSB from 0x40.
unsafe fn read_pit_channel0_count() -> u16 {
    // Latch counter command: bits [7:6]=00 (channel 0), bits [5:4]=00 (latch), bits [3:1]=000 (mode), bit 0=0
    outb(0x43, 0x00);
    let lsb = inb(0x40) as u16;
    let msb = inb(0x40) as u16;
    (msb << 8) | lsb
}

/// Read PIT Channel 0 status via Read-Back command.
/// Writes 0xC2 to 0x43: bits [7:6]=11 (read-back), bit 5=0 (latch count),
/// bit 4=1 (latch status), bit 1=1 (select channel 0).
/// Returns the status byte read from 0x40.
unsafe fn read_pit_channel0_status() -> u8 {
    // 0xC2 = 1100_0010: read-back, latch status only, channel 0
    outb(0x43, 0xC2);
    inb(0x40)
}

/// Decode status bits [3:1] (mode field) into a consciousness score.
fn mode_bits_to_sense(status: u8) -> u16 {
    let mode = (status >> 1) & 0x07;
    match mode {
        0 => 250, // interrupt on terminal count
        2 => 800, // rate generator (standard system timer)
        3 => 600, // square wave generator
        _ => 400, // modes 1, 4, 5 or reserved
    }
}

pub fn init() {
    serial_println!("[pit_mode] PIT Channel 0 system timer heartbeat sense online");
}

pub fn tick(age: u32) {
    let mut state = MODULE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Gate: update every 8 ticks
    if age % 8 != 0 {
        return;
    }

    // --- Read current counter value ---
    let count = unsafe { read_pit_channel0_count() };

    // timer_count: scale 0-65535 down to 0-1000 (u32 intermediate to avoid overflow)
    let raw_count_scaled = ((count as u32) * 1000 / 65535) as u16;

    // timer_phase: low 8 bits of count scaled to 0-1000
    let phase = ((count & 0xFF) as u16) * 1000 / 255;

    // channel0_pulse: bit 8 of count toggles at ~2.3 kHz — map to 0 or 1000
    let pulse = if (count >> 8) & 1 == 1 { 1000u16 } else { 0u16 };

    // --- Read operating mode via read-back status ---
    let status = unsafe { read_pit_channel0_status() };
    let mode_raw = mode_bits_to_sense(status);

    // --- Apply EMA to timer_count and mode_sense ---
    // EMA formula: (old * 7 + signal) / 8
    let new_timer_count = ((state.timer_count as u32) * 7 + raw_count_scaled as u32) / 8;
    let new_mode_sense  = ((state.mode_sense  as u32) * 7 + mode_raw           as u32) / 8;

    state.timer_count    = new_timer_count as u16;
    state.timer_phase    = phase;
    state.channel0_pulse = pulse;
    state.mode_sense     = new_mode_sense as u16;

    if state.tick_count % 512 == 0 {
        serial_println!(
            "[pit_mode] count={} timer_count={} phase={} mode_sense={} pulse={}",
            count, state.timer_count, state.timer_phase, state.mode_sense, state.channel0_pulse
        );
    }
}

pub fn get_timer_count() -> u16 {
    MODULE.lock().timer_count
}

pub fn get_timer_phase() -> u16 {
    MODULE.lock().timer_phase
}

pub fn get_mode_sense() -> u16 {
    MODULE.lock().mode_sense
}

pub fn get_channel0_pulse() -> u16 {
    MODULE.lock().channel0_pulse
}

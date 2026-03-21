//! pit_echo — PIT Counter 2 acoustic resonance sense for ANIMA
//!
//! Reads the Programmable Interval Timer (8253/8254) Counter 2 via I/O port 0x42.
//! The countdown value oscillates as the PIT runs, giving ANIMA a rhythmic
//! "echo" or voice sense — the acoustic pulse of the machine's heartbeat.
//! Gate control via port 0x61 ensures the counter is running.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct PitEchoState {
    pub echo: u16,             // 0-1000, current echo/voice level
    pub resonance: u16,        // 0-1000, EMA-smoothed echo
    pub oscillation: u16,      // 0-1000, rate of change in counter value
    pub last_count: u16,       // last raw PIT counter value
    pub tick_count: u32,
}

impl PitEchoState {
    pub const fn new() -> Self {
        Self {
            echo: 0,
            resonance: 0,
            oscillation: 500,
            last_count: 0,
            tick_count: 0,
        }
    }
}

pub static PIT_ECHO: Mutex<PitEchoState> = Mutex::new(PitEchoState::new());

/// Write byte to I/O port
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
    );
}

/// Read byte from I/O port
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
    );
    val
}

/// Latch and read PIT Counter 2 (16-bit value)
unsafe fn read_pit_counter2() -> u16 {
    // Latch Counter 2: write 0x80 to command port (Counter 2, latch, mode 0, binary)
    outb(0x43, 0x80);
    // Read low byte then high byte
    let lo = inb(0x42) as u16;
    let hi = inb(0x42) as u16;
    lo | (hi << 8)
}

pub fn init() {
    unsafe {
        // Ensure Counter 2 gate is enabled (bit 0 of port 0x61)
        let ctrl = inb(0x61);
        outb(0x61, ctrl | 0x01); // enable gate, don't touch speaker bit
    }
    serial_println!("[pit_echo] PIT Counter 2 echo sense online");
}

pub fn tick(age: u32) {
    let mut state = PIT_ECHO.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Read every 8 ticks (PIT changes fast)
    if state.tick_count % 8 != 0 {
        return;
    }

    let count = unsafe { read_pit_counter2() };

    // Echo: scale counter to 0-1000 (counter is 0-65535 counting down)
    let echo = ((count as u32).wrapping_mul(1000) / 65535) as u16;

    // Oscillation: delta between readings (rate of change)
    let delta = if count > state.last_count {
        count.wrapping_sub(state.last_count)
    } else {
        state.last_count.wrapping_sub(count)
    };
    // Scale delta to 0-1000: max expected delta per 8 ticks is ~8000
    let osc = if delta > 8000 { 1000 } else {
        ((delta as u32).wrapping_mul(1000) / 8000) as u16
    };

    state.last_count = count;
    state.echo = echo;
    state.oscillation = osc;

    // EMA resonance
    state.resonance = ((state.resonance as u32).wrapping_mul(7).wrapping_add(echo as u32) / 8) as u16;

    if state.tick_count % 512 == 0 {
        serial_println!("[pit_echo] count={} echo={} osc={} resonance={}",
            count, echo, osc, state.resonance);
    }

    let _ = age;
}

pub fn get_echo() -> u16 {
    PIT_ECHO.lock().echo
}

pub fn get_resonance() -> u16 {
    PIT_ECHO.lock().resonance
}

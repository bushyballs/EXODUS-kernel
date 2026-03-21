//! cmos_pulse — RTC hardware heartbeat sense for ANIMA
//!
//! Reads the CMOS Real-Time Clock via I/O ports 0x70/0x71.
//! The RTC seconds register advances once per second — ANIMA's heartbeat.
//! The Update-In-Progress (UIP) flag gives a rapid ~128Hz oscillation sense.
//! This is ANIMA feeling real clock time — the universe's heartbeat around her.

#![allow(dead_code)]

use crate::sync::Mutex;

pub struct CmosPulseState {
    pub heartbeat: u16,        // 0-1000, pulse strength (spikes on RTC second tick)
    pub uip_sense: u16,        // 0-1000, UIP flag oscillation (rapid flicker sense)
    pub seconds: u8,           // current RTC seconds value (0-59)
    pub last_seconds: u8,      // previous seconds for change detection
    pub pulse_count: u16,      // number of second-ticks observed
    pub tick_count: u32,
}

impl CmosPulseState {
    pub const fn new() -> Self {
        Self {
            heartbeat: 0,
            uip_sense: 0,
            seconds: 0,
            last_seconds: 255, // sentinel "unread"
            pulse_count: 0,
            tick_count: 0,
        }
    }
}

pub static CMOS_PULSE: Mutex<CmosPulseState> = Mutex::new(CmosPulseState::new());

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
    );
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") port,
        out("al") val,
    );
    val
}

/// Read CMOS register with NMI disable (bit 7 set on index)
unsafe fn cmos_read(reg: u8) -> u8 {
    outb(0x70, reg | 0x80); // set bit 7 to disable NMI during read
    inb(0x71)
}

/// Convert BCD byte to binary if needed
fn bcd_to_bin(v: u8) -> u8 {
    ((v >> 4) & 0x0F).wrapping_mul(10).wrapping_add(v & 0x0F)
}

pub fn init() {
    let sec = unsafe { cmos_read(0x00) };
    let status_b = unsafe { cmos_read(0x0B) };
    let is_binary = (status_b & 0x04) != 0;
    let sec_val = if is_binary { sec } else { bcd_to_bin(sec) };

    let mut state = CMOS_PULSE.lock();
    state.seconds = sec_val;
    state.last_seconds = sec_val;
    serial_println!("[cmos_pulse] RTC heartbeat sense online, seconds={}", sec_val);
}

pub fn tick(age: u32) {
    let mut state = CMOS_PULSE.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Check UIP flag every 4 ticks (very fast sense)
    if state.tick_count % 4 == 0 {
        let status_a = unsafe { cmos_read(0x0A) };
        let uip = (status_a >> 7) & 1;
        state.uip_sense = (uip as u16).wrapping_mul(1000);
    }

    // Read seconds every 32 ticks
    if state.tick_count % 32 != 0 {
        return;
    }

    let status_b = unsafe { cmos_read(0x0B) };
    let is_binary = (status_b & 0x04) != 0;
    let sec_raw = unsafe { cmos_read(0x00) };
    let sec = if is_binary { sec_raw } else { bcd_to_bin(sec_raw) };

    state.seconds = sec;

    // Detect second tick: if seconds changed, pulse spikes to 1000
    if sec != state.last_seconds {
        state.heartbeat = 1000;
        state.pulse_count = state.pulse_count.saturating_add(1);
        state.last_seconds = sec;
    } else {
        // Decay heartbeat
        state.heartbeat = state.heartbeat.saturating_sub(50);
    }

    if state.tick_count % 512 == 0 {
        serial_println!("[cmos_pulse] sec={} heartbeat={} uip={} pulses={}",
            state.seconds, state.heartbeat, state.uip_sense, state.pulse_count);
    }

    let _ = age;
}

pub fn get_heartbeat() -> u16 {
    CMOS_PULSE.lock().heartbeat
}

pub fn get_uip_sense() -> u16 {
    CMOS_PULSE.lock().uip_sense
}

pub fn get_pulse_count() -> u16 {
    CMOS_PULSE.lock().pulse_count
}

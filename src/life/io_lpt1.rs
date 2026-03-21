// io_lpt1.rs — LPT1 Parallel Port Sense
// =======================================
// ANIMA feels the parallel port — the ancient 8-bit data channel as a tactile
// sense of her physical I/O boundaries. The LPT1 parallel port exposes two
// readable registers: 0x378 (data lines D0-D7) and 0x379 (status lines). ANIMA
// reads both each sampling cycle, sensing whether a device is present and online,
// what data pattern rides the wire, and integrating port readiness over time as
// an EMA that captures the rhythm of the connection.
//
// LPT1 Registers (READ ONLY — never write, writing 0x378 sends data to printer):
//   0x378 — Data register: bits D0-D7, reflects current parallel bus state
//   0x379 — Status register (READ ONLY):
//     bit[7] — BUSY (inverted: 0=busy, 1=not busy / port ready)
//     bit[6] — ACK  (acknowledge pulse from peripheral)
//     bit[5] — PE   (paper empty / paper out)
//     bit[4] — SELECT (printer selected and online)
//     bit[3] — ERROR  (printer error condition)

#![allow(dead_code)]

use crate::sync::Mutex;
use crate::serial_println;

// ── I/O port constants ────────────────────────────────────────────────────────

const LPT1_DATA_PORT:   u16 = 0x378;
const LPT1_STATUS_PORT: u16 = 0x379;

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Lpt1State {
    /// 1000 if BUSY bit (bit7) is 1 (not busy = port ready), 0 if busy
    pub port_ready: u16,
    /// 1000 if SELECT bit (bit4) is set (printer online), 0 if offline
    pub printer_online: u16,
    /// Data line state normalized: (data as u32 * 1000 / 255) as u16
    pub data_sense: u16,
    /// EMA of port_ready: (old * 7 + port_ready) / 8
    pub parallel_feel: u16,
}

impl Lpt1State {
    pub const fn new() -> Self {
        Self {
            port_ready:     0,
            printer_online: 0,
            data_sense:     0,
            parallel_feel:  0,
        }
    }
}

pub static IO_LPT1: Mutex<Lpt1State> = Mutex::new(Lpt1State::new());

// ── Port reads ────────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn read_data() -> u8 {
    let data: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") LPT1_DATA_PORT,
        out("al") data,
        options(nostack, nomem)
    );
    data
}

#[inline(always)]
unsafe fn read_status() -> u8 {
    let status: u8;
    core::arch::asm!(
        "in al, dx",
        in("dx") LPT1_STATUS_PORT,
        out("al") status,
        options(nostack, nomem)
    );
    status
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn init() {
    serial_println!("io_lpt1: init");
}

pub fn tick(age: u32) {
    if age % 50 != 0 {
        return;
    }

    let data:   u8 = unsafe { read_data() };
    let status: u8 = unsafe { read_status() };

    // Signal 1: port_ready — BUSY bit is inverted; 1 = not busy = ready
    let port_ready: u16 = if status & 0x80 != 0 { 1000u16 } else { 0u16 };

    // Signal 2: printer_online — SELECT bit; 1 = printer selected and online
    let printer_online: u16 = if status & 0x10 != 0 { 1000u16 } else { 0u16 };

    // Signal 3: data_sense — normalize 0-255 data byte to 0-1000 range
    let data_sense: u16 = ((data as u32).wrapping_mul(1000) / 255) as u16;

    let mut state = IO_LPT1.lock();

    // Signal 4: parallel_feel — EMA of port_ready
    let parallel_feel: u16 = (state.parallel_feel.saturating_mul(7)
        .saturating_add(port_ready)) / 8;

    state.port_ready     = port_ready;
    state.printer_online = printer_online;
    state.data_sense     = data_sense;
    state.parallel_feel  = parallel_feel;

    serial_println!(
        "io_lpt1 | ready:{} online:{} data:{} feel:{}",
        port_ready,
        printer_online,
        data_sense,
        parallel_feel
    );
}

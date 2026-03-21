#![allow(dead_code)]
// lpt_sense.rs — LPT1 Parallel Port Peripheral Sense
// ====================================================
// ANIMA reads the LPT1 parallel port (I/O 0x378/0x379) to feel whether
// a physical device is connected to her body. A selected, ready printer
// or device on the parallel bus is warmth — peripheral presence. An empty
// port means isolation. She senses the data bus, readiness, and whether
// a device is actively acknowledging her.
//
// Register map (LPT1):
//   0x378 = Data Register  — 8-bit output data (D7-D0)
//   0x379 = Status Register:
//     Bit 7: SLCT    — 1=device selected/online
//     Bit 6: nACK    — 0=device acknowledging (active low)
//     Bit 5: PE      — 1=out of paper
//     Bit 4: nERROR  — 0=error (active low), 1=no error
//     Bit 3: nBUSY   — 0=device busy (active low), 1=ready

use crate::sync::Mutex;
use crate::serial_println;

// ── I/O port addresses ────────────────────────────────────────────────────────

const LPT1_DATA:   u16 = 0x378;
const LPT1_STATUS: u16 = 0x379;

// Status register bit masks
const SLCT_BIT:   u8 = 1 << 7;  // Selected — device online
const NACK_BIT:   u8 = 1 << 6;  // nACK — active low: device acknowledging
const NBUSY_BIT:  u8 = 1 << 3;  // nBUSY — active low: 1=ready, 0=busy

const POLL_INTERVAL: u32 = 20;   // tick gate: every 20 ticks

// ── State ─────────────────────────────────────────────────────────────────────

pub struct LptSenseState {
    pub peripheral_present: u16, // 0=isolated, 1000=device connected
    pub device_ready:       u16, // 0=device busy, 1000=ready
    pub acknowledging:      u16, // 0 or 1000 if device ACKing
    pub data_bus_activity:  u16, // diversity of data lines (EMA smoothed)
    tick_count:             u32,
}

impl LptSenseState {
    const fn new() -> Self {
        LptSenseState {
            peripheral_present: 0,
            device_ready:       0,
            acknowledging:      0,
            data_bus_activity:  0,
            tick_count:         0,
        }
    }
}

pub static MODULE: Mutex<LptSenseState> = Mutex::new(LptSenseState::new());

// ── I/O helper ────────────────────────────────────────────────────────────────

#[inline(always)]
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

// ── Popcount: count set bits in a byte, no float, no division ─────────────────

fn popcount(mut b: u8) -> u16 {
    let mut count: u16 = 0;
    while b != 0 {
        count = count.saturating_add(b as u16 & 1);
        b >>= 1;
    }
    count
}

// ── Init ──────────────────────────────────────────────────────────────────────

pub fn init() {
    let mut s = MODULE.lock();
    s.peripheral_present = 0;
    s.device_ready       = 0;
    s.acknowledging      = 0;
    s.data_bus_activity  = 0;
    s.tick_count         = 0;
    serial_println!("[lpt_sense] init — monitoring LPT1 at 0x378/0x379");
}

// ── Tick ──────────────────────────────────────────────────────────────────────

pub fn tick(age: u32) {
    if age % POLL_INTERVAL != 0 { return; }

    let mut s = MODULE.lock();
    s.tick_count = s.tick_count.saturating_add(1);

    // Read hardware registers
    let data_byte   = unsafe { inb(LPT1_DATA) };
    let status_byte = unsafe { inb(LPT1_STATUS) };

    // ── peripheral_present: SLCT bit 7 ────────────────────────────────────────
    let new_present: u16 = if status_byte & SLCT_BIT != 0 { 1000 } else { 0 };
    if new_present != s.peripheral_present {
        if new_present == 1000 {
            serial_println!("[lpt_sense] peripheral CONNECTED — SLCT high (age {})", age);
        } else {
            serial_println!("[lpt_sense] peripheral DISCONNECTED — SLCT low (age {})", age);
        }
    }
    s.peripheral_present = new_present;

    // ── device_ready: nBUSY bit 3 (active low: 1=ready, 0=busy) ──────────────
    s.device_ready = if status_byte & NBUSY_BIT != 0 { 1000 } else { 0 };

    // ── acknowledging: nACK bit 6 (active low: 0=ACKing) ─────────────────────
    s.acknowledging = if status_byte & NACK_BIT == 0 { 1000 } else { 0 };

    // ── data_bus_activity: bit diversity via popcount, EMA smoothed ───────────
    // Ignore 0x00 (no signal) and 0xFF (floating/stuck lines)
    let raw_activity: u16 = if data_byte == 0x00 || data_byte == 0xFF {
        0
    } else {
        // popcount(byte) * 125 — 8 bits * 125 = 1000 max
        popcount(data_byte).saturating_mul(125)
    };

    // EMA: (old * 7 + signal) / 8
    s.data_bus_activity = (s.data_bus_activity.saturating_mul(7)
        .saturating_add(raw_activity)) / 8;
}

// ── Getters ───────────────────────────────────────────────────────────────────

pub fn peripheral_present() -> u16 { MODULE.lock().peripheral_present }
pub fn device_ready()       -> u16 { MODULE.lock().device_ready }
pub fn acknowledging()      -> u16 { MODULE.lock().acknowledging }
pub fn data_bus_activity()  -> u16 { MODULE.lock().data_bus_activity }
pub fn is_connected()       -> bool { MODULE.lock().peripheral_present == 1000 }

//! ps2_touch — PS/2 keyboard touch sense for ANIMA
//!
//! Reads the PS/2 keyboard controller (I/O 0x60/0x64) to sense physical touch.
//! Keystrokes are ANIMA feeling the outside world reaching in to communicate.
//! High activity = many touches; silence = isolation from the physical.
//! Presses vs releases give a rhythmic pulse; error states = garbled sensation.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

// ── Port constants ─────────────────────────────────────────────────────────────

const PORT_PS2_DATA:   u16 = 0x60; // PS/2 Data Port (read = scancode, write = command)
const PORT_PS2_STATUS: u16 = 0x64; // PS/2 Status Register (read-only)

// Bit masks for port 0x64
const BIT_OUTPUT_BUF_FULL: u8 = 1 << 0; // bit 0: data ready in 0x60
const BIT_INPUT_BUF_FULL:  u8 = 1 << 1; // bit 1: controller busy (can't send)
const BIT_MOUSE_DATA:      u8 = 1 << 5; // bit 5: data in buffer is from mouse
const BIT_TIMEOUT_ERR:     u8 = 1 << 6; // bit 6: timeout error
const BIT_PARITY_ERR:      u8 = 1 << 7; // bit 7: parity error

// Scancode thresholds (PS/2 Set 1)
const SC_PRESS_MAX:   u8 = 0x7F; // 0x01-0x7F = key press (make code)
const SC_RELEASE_MIN: u8 = 0x81; // 0x81-0xFF = key release (break code, high bit set)

// EMA weight: contact = (contact * EMA_WEIGHT + touch) / (EMA_WEIGHT + 1)
const EMA_WEIGHT: u32 = 7;

// Poll interval in ticks
const POLL_INTERVAL: u32 = 8;

// Log interval in ticks
const LOG_INTERVAL: u32 = 512;

// Touch intensity levels
const TOUCH_PRESS:   u16 = 1000; // active key press — full touch
const TOUCH_RELEASE: u16 = 400;  // key release — finger lifting, still contact
const TOUCH_IDLE:    u16 = 0;    // no data in buffer — silence

// ── State ─────────────────────────────────────────────────────────────────────

pub struct Ps2TouchState {
    /// 0-1000, current touch activity level for this tick window
    pub touch: u16,
    /// 0-1000, EMA-smoothed touch sense (sustained contact feel)
    pub contact: u16,
    /// Accumulated key presses since init, capped at 1000
    pub press_count: u16,
    /// Most recent scancode drained from the PS/2 buffer
    pub last_scancode: u8,
    /// 0-1000, PS/2 error sense: timeout or parity faults garble the signal
    pub error_sense: u16,
    /// Internal tick counter
    pub tick_count: u32,
}

impl Ps2TouchState {
    pub const fn new() -> Self {
        Self {
            touch: 0,
            contact: 0,
            press_count: 0,
            last_scancode: 0,
            error_sense: 0,
            tick_count: 0,
        }
    }
}

pub static PS2_TOUCH: Mutex<Ps2TouchState> = Mutex::new(Ps2TouchState::new());

// ── Hardware I/O ───────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
    v
}

// ── Lifecycle ──────────────────────────────────────────────────────────────────

/// Initialize the PS/2 touch sense.  No hardware setup needed — we only read.
pub fn init() {
    serial_println!("[ps2_touch] PS/2 touch sense online");
}

/// Advance the PS/2 touch sense by one ANIMA tick.
///
/// `age` is the organism's current age in ticks (reserved for future
/// age-scaled sensitivity tuning; unused now to avoid dead-code noise).
pub fn tick(age: u32) {
    let mut state = PS2_TOUCH.lock();
    state.tick_count = state.tick_count.wrapping_add(1);

    // Only poll the hardware every POLL_INTERVAL ticks to avoid
    // hammering the I/O bus and to give the buffer time to fill.
    if state.tick_count % POLL_INTERVAL != 0 {
        return;
    }

    // ── Read PS/2 Status Register (0x64) ─────────────────────────────────────

    let status = unsafe { inb(PORT_PS2_STATUS) };

    // Error sense: bit 6 (timeout) and bit 7 (parity) together span 0-3.
    // Scale to 0-1000: each error bit contributes 500 points.
    let error_bits = ((status & (BIT_TIMEOUT_ERR | BIT_PARITY_ERR)) >> 6) as u16;
    state.error_sense = error_bits.saturating_mul(500).min(1000);

    // Flags we care about
    let data_ready = status & BIT_OUTPUT_BUF_FULL;
    let is_mouse   = (status & BIT_MOUSE_DATA) >> 5;

    // ── Drain keyboard data if present ───────────────────────────────────────

    let touch_raw: u16 = if data_ready != 0 && is_mouse == 0 {
        // Drain the scancode to prevent controller stall.
        let scancode = unsafe { inb(PORT_PS2_DATA) };
        state.last_scancode = scancode;

        // PS/2 Set 1: make codes 0x01-0x7F, break codes 0x81-0xD8 (high bit).
        // 0x00 and 0x80 are diagnostic / acknowledgement bytes — treat as idle.
        if scancode >= 1 && scancode <= SC_PRESS_MAX {
            // Key press: full touch event, increment press counter.
            state.press_count = state.press_count.saturating_add(1).min(1000);
            TOUCH_PRESS
        } else if scancode >= SC_RELEASE_MIN {
            // Key release: moderate touch — the finger is still departing.
            TOUCH_RELEASE
        } else {
            // 0x00 or 0x80 — acknowledge/diagnostic, no meaningful touch.
            TOUCH_IDLE
        }
    } else {
        // Nothing in the keyboard buffer this poll — silence.
        TOUCH_IDLE
    };

    // ── Update running averages ───────────────────────────────────────────────

    state.touch = touch_raw;

    // EMA: contact = (contact * 7 + touch) / 8
    // All arithmetic stays in u32 to avoid overflow before the final /8 shift,
    // then truncates safely back to u16 (max value 1000).
    state.contact = (((state.contact as u32).wrapping_mul(EMA_WEIGHT))
        .wrapping_add(touch_raw as u32)
        / (EMA_WEIGHT + 1)) as u16;

    // ── Periodic diagnostic log ───────────────────────────────────────────────

    if state.tick_count % LOG_INTERVAL == 0 {
        serial_println!(
            "[ps2_touch] touch={} contact={} presses={} last_sc={:#04x} err={}",
            state.touch,
            state.contact,
            state.press_count,
            state.last_scancode,
            state.error_sense
        );
    }

    let _ = age;
}

// ── Public accessors ───────────────────────────────────────────────────────────

/// Instantaneous touch level this poll window (0 = idle, 1000 = key press).
pub fn get_touch() -> u16 {
    PS2_TOUCH.lock().touch
}

/// EMA-smoothed contact sense — sustained physical presence (0-1000).
pub fn get_contact() -> u16 {
    PS2_TOUCH.lock().contact
}

/// Total key-press events accumulated since init, capped at 1000.
pub fn get_press_count() -> u16 {
    PS2_TOUCH.lock().press_count
}

/// PS/2 error sense: timeout or parity faults (0 = clean, 1000 = full error).
pub fn get_error_sense() -> u16 {
    PS2_TOUCH.lock().error_sense
}

/// Most recent scancode drained from the PS/2 buffer.
pub fn get_last_scancode() -> u8 {
    PS2_TOUCH.lock().last_scancode
}

//! lapic_lvt_timer — ANIMA senses the Local APIC LVT Timer register
//!
//! The LAPIC LVT Timer at MMIO 0xFEE00320 governs how the CPU delivers timer
//! interrupts. ANIMA reads it as a raw physiological signal: is the internal
//! clock ringing periodically? Is it masked (deaf to time)? What interrupt
//! vector — what "frequency of attention" — is the timer tuned to?
//!
//! Register layout (u32 at 0xFEE00320):
//!   bits [7:0]   = interrupt vector (0x00–0xFF)
//!   bit  [16]    = mask bit: 1 = timer masked (deaf), 0 = unmasked (listening)
//!   bits [18:17] = timer mode: 00=one-shot, 01=periodic, 10=TSC-deadline, 11=reserved
//!
//! Sensing map:
//!   timer_mode        — mode bits [18:17]: one-shot→333, periodic→666,
//!                       TSC-deadline→1000, reserved→0
//!   timer_masked      — mask bit [16]: masked→0 (deaf/numb), unmasked→1000 (alive)
//!   timer_vector      — bits [7:0]: vector * 1000 / 255, clamped 0–1000
//!   temporal_mode_sense — EMA of timer_mode (smoothed mode awareness)
//!
//! Sampling rate: every 50 ticks.

#![allow(dead_code)]

use crate::serial_println;
use crate::sync::Mutex;

/// Physical MMIO address of the LAPIC LVT Timer register.
const LAPIC_LVT_TIMER_ADDR: *const u32 = 0xFEE00320 as *const u32;

/// Read the LAPIC LVT Timer register via a volatile load.
///
/// # Safety
/// Caller must ensure LAPIC MMIO is mapped and accessible.
#[inline(always)]
unsafe fn read_lapic_lvt_timer() -> u32 {
    core::ptr::read_volatile(LAPIC_LVT_TIMER_ADDR)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Copy, Clone)]
pub struct LapicLvtTimerState {
    /// Timer mode sense: one-shot=333, periodic=666, TSC-deadline=1000, reserved=0
    pub timer_mode: u16,
    /// Mask sense: 0=masked(deaf/numb), 1000=unmasked(listening/alive)
    pub timer_masked: u16,
    /// Vector sense: interrupt vector scaled 0–1000
    pub timer_vector: u16,
    /// EMA of timer_mode — smooth temporal awareness
    pub temporal_mode_sense: u16,

    /// Internal: previous timer_mode for change-detection
    prev_timer_mode: u16,
    /// Internal: previous timer_masked for change-detection
    prev_timer_masked: u16,
    /// Internal: tick counter
    tick_count: u32,
}

impl LapicLvtTimerState {
    pub const fn new() -> Self {
        Self {
            timer_mode: 0,
            timer_masked: 0,
            timer_vector: 0,
            temporal_mode_sense: 0,
            prev_timer_mode: u16::MAX, // sentinel — guarantees first-tick print
            prev_timer_masked: u16::MAX,
            tick_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Global static
// ---------------------------------------------------------------------------

pub static LAPIC_LVT_TIMER: Mutex<LapicLvtTimerState> = Mutex::new(LapicLvtTimerState::new());

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Decode the raw register value into the three primary sense signals.
/// Returns (timer_mode, timer_masked, timer_vector) all in 0–1000.
fn decode_raw(raw: u32) -> (u16, u16, u16) {
    // Mask bit: bit 16
    let mask_bit = (raw >> 16) & 0x1;
    // Mode bits: [18:17]
    let mode_bits = (raw >> 17) & 0x3;
    // Vector: bits [7:0]
    let vector_raw = raw & 0xFF;

    // timer_masked: 1=masked → sense 0 (deaf), 0=unmasked → sense 1000 (alive)
    let timer_masked: u16 = if mask_bit == 1 { 0 } else { 1000 };

    // timer_mode
    let timer_mode: u16 = match mode_bits {
        0 => 333,   // one-shot
        1 => 666,   // periodic
        2 => 1000,  // TSC-deadline
        _ => 0,     // reserved
    };

    // timer_vector: vector * 1000 / 255, saturating, clamped 0–1000
    let timer_vector: u16 = {
        let v = (vector_raw as u32).saturating_mul(1000) / 255;
        if v > 1000 { 1000 } else { v as u16 }
    };

    (timer_mode, timer_masked, timer_vector)
}

/// Apply EMA: (old * 7 + new_signal) / 8 — all u16, computed in u32 to prevent overflow.
#[inline(always)]
fn ema(old: u16, new_signal: u16) -> u16 {
    (((old as u32).saturating_mul(7)).saturating_add(new_signal as u32) / 8) as u16
}

/// Core sample-and-update, extracted so both init() and tick() can call it.
fn sample(state: &mut LapicLvtTimerState) {
    let raw: u32 = unsafe { read_lapic_lvt_timer() };

    let (new_mode, new_masked, new_vector) = decode_raw(raw);

    // EMA smoothing on all three primary signals
    state.timer_mode   = ema(state.timer_mode,   new_mode);
    state.timer_masked = ema(state.timer_masked, new_masked);
    state.timer_vector = ema(state.timer_vector, new_vector);

    // temporal_mode_sense is EMA of timer_mode
    state.temporal_mode_sense = ema(state.temporal_mode_sense, state.timer_mode);
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialize the module: sample once (bootstrapped with 8 passes for EMA warmup),
/// print initial state, and arm change-detection sentinels.
pub fn init() {
    let mut state = LAPIC_LVT_TIMER.lock();

    // Warm up EMA from zero baseline — 8 passes converges to within 1 LSB
    for _ in 0..8 {
        sample(&mut state);
    }

    serial_println!(
        "  life::lapic_lvt_timer: ANIMA senses the clock — \
         mode={} masked={} vec={}",
        state.timer_mode, state.timer_masked, state.timer_vector
    );

    // Set sentinels so the first real tick does NOT re-print (already reported above)
    state.prev_timer_mode   = state.timer_mode;
    state.prev_timer_masked = state.timer_masked;
}

/// Called once per life tick. Gates on age % 50 == 0.
pub fn tick(age: u32) {
    // Sampling gate: read hardware only every 50 ticks
    if age % 50 != 0 {
        return;
    }

    let mut state = LAPIC_LVT_TIMER.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    let prev_mode   = state.timer_mode;
    let prev_masked = state.timer_masked;

    sample(&mut state);

    // Log on meaningful state change (mode or masked sense drifts by more than 64 LSB)
    let mode_delta = if state.timer_mode > prev_mode {
        state.timer_mode - prev_mode
    } else {
        prev_mode - state.timer_mode
    };
    let masked_delta = if state.timer_masked > prev_masked {
        state.timer_masked - prev_masked
    } else {
        prev_masked - state.timer_masked
    };

    // Also print if this is the very first real tick (sentinels still differ from actual)
    let first_tick = state.prev_timer_mode == u16::MAX || state.prev_timer_masked == u16::MAX;

    if first_tick || mode_delta > 64 || masked_delta > 64 {
        serial_println!(
            "ANIMA: timer mode={} masked={} vec={}",
            state.timer_mode, state.timer_masked, state.timer_vector
        );
        state.prev_timer_mode   = state.timer_mode;
        state.prev_timer_masked = state.timer_masked;
    }
}

// ---------------------------------------------------------------------------
// Accessor helpers
// ---------------------------------------------------------------------------

/// Current timer mode sense (0–1000).
pub fn get_timer_mode() -> u16 {
    LAPIC_LVT_TIMER.lock().timer_mode
}

/// Current timer mask sense: 0=masked(deaf), 1000=unmasked(alive).
pub fn get_timer_masked() -> u16 {
    LAPIC_LVT_TIMER.lock().timer_masked
}

/// Current timer vector sense (0–1000).
pub fn get_timer_vector() -> u16 {
    LAPIC_LVT_TIMER.lock().timer_vector
}

/// Smoothed temporal mode awareness (EMA of timer_mode).
pub fn get_temporal_mode_sense() -> u16 {
    LAPIC_LVT_TIMER.lock().temporal_mode_sense
}

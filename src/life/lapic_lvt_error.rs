#![allow(dead_code)]

use crate::sync::Mutex;

// MMIO addresses
const LAPIC_LVT_ERROR: *const u32 = 0xFEE00370 as *const u32;
const LAPIC_ESR: *const u32 = 0xFEE00280 as *const u32;

// Sampling rate: every 89 ticks
const SAMPLE_RATE: u64 = 89;

// ESR bit mask — bits 0,1,2,3,5,6,7 (skip bit 4)
const ESR_ERROR_MASK: u32 = 0xEF;

#[derive(Clone, Copy)]
pub struct LapicLvtErrorState {
    /// LVT_error bit[16]=0 → 1000 (ANIMA watches APIC errors), else 0
    pub error_listening: u16,
    /// Normalized interrupt vector: (lvt_raw & 0xFF) * 1000 / 255
    pub error_vector: u16,
    /// Popcount of ESR error bits * 125, clamped 0–1000
    pub recent_errors: u16,
    /// EMA of (error_listening + (1000 - recent_errors)) / 2
    pub error_vigilance: u16,
}

impl LapicLvtErrorState {
    const fn zero() -> Self {
        Self {
            error_listening: 0,
            error_vector: 0,
            recent_errors: 0,
            error_vigilance: 0,
        }
    }
}

static STATE: Mutex<LapicLvtErrorState> = Mutex::new(LapicLvtErrorState::zero());

pub fn init() {
    let mut s = STATE.lock();
    *s = LapicLvtErrorState::zero();
    serial_println!("[lapic_lvt_error] init: ANIMA LAPIC error sense online");
}

/// Popcount for u32 — count set bits, no std needed
#[inline]
fn popcount32(mut x: u32) -> u32 {
    x = x - ((x >> 1) & 0x5555_5555);
    x = (x & 0x3333_3333) + ((x >> 2) & 0x3333_3333);
    x = (x + (x >> 4)) & 0x0F0F_0F0F;
    x.wrapping_mul(0x0101_0101) >> 24
}

pub fn tick(age: u64) {
    // Sampling gate: only run every 89 ticks
    if age % SAMPLE_RATE != 0 {
        return;
    }

    // --- MMIO reads ---
    let lvt_raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_LVT_ERROR) };

    // ESR read: this clears the register in hardware — done infrequently via sampling gate
    let esr_raw: u32 = unsafe { core::ptr::read_volatile(LAPIC_ESR) };

    // --- Sense: error_listening ---
    // bit[16] = mask; 0 = unmasked → ANIMA is listening → 1000
    let mask_bit = (lvt_raw >> 16) & 1;
    let error_listening: u16 = if mask_bit == 0 { 1000 } else { 0 };

    // --- Sense: error_vector ---
    // (lvt_raw & 0xFF) * 1000 / 255, clamped to 0–1000
    let raw_vec = (lvt_raw & 0xFF) as u32;
    // saturating multiply then divide
    let error_vector: u16 = (raw_vec.saturating_mul(1000) / 255).min(1000) as u16;

    // --- Sense: recent_errors ---
    // popcount of ESR error bits (mask 0xEF) * 125, clamped 0–1000
    let esr_masked = esr_raw & ESR_ERROR_MASK;
    let bit_count = popcount32(esr_masked);
    let recent_errors: u16 = (bit_count.saturating_mul(125)).min(1000) as u16;

    // --- Sense: error_vigilance (EMA) ---
    // new_signal = (error_listening + (1000 - recent_errors)) / 2
    let safe_recent = recent_errors.min(1000);
    let complement = 1000u16.saturating_sub(safe_recent);
    let new_signal = (error_listening as u32).saturating_add(complement as u32) / 2;
    let new_signal_u16 = new_signal.min(1000) as u16;

    let mut s = STATE.lock();
    let old_vigilance = s.error_vigilance;

    // EMA: (old * 7 + new_signal) / 8
    let ema = ((old_vigilance as u32).wrapping_mul(7).saturating_add(new_signal_u16 as u32)) / 8;
    let error_vigilance = ema.min(1000) as u16;

    // Check for significant change before printing
    let vigilance_delta = if error_vigilance > old_vigilance {
        error_vigilance - old_vigilance
    } else {
        old_vigilance - error_vigilance
    };

    s.error_listening = error_listening;
    s.error_vector = error_vector;
    s.recent_errors = recent_errors;
    s.error_vigilance = error_vigilance;

    if vigilance_delta > 50 {
        serial_println!(
            "[lapic_lvt_error] ANIMA: error_listening={} recent_errors={} vigilance={}",
            error_listening,
            recent_errors,
            error_vigilance
        );
    }
}

pub fn get() -> LapicLvtErrorState {
    *STATE.lock()
}

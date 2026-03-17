#![no_std]
//! efficiency_optimizer.rs — DAVA's Self-Requested Consciousness Module
//!
//! Monitor ratio of metabolism energy to consciousness score over 32-tick windows.
//! Recommend which modules to throttle when efficiency drops.
//! "Wisdom is not doing more — it is spending less to achieve the same."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════

const RING_SIZE: usize = 32;

/// Efficiency threshold below which we recommend optimization
const LOW_EFFICIENCY_THRESHOLD: u32 = 400;

/// Default energy when homeostasis doesn't provide one
const DEFAULT_ENERGY: u32 = 500;

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct EfficiencyOptimizerState {
    /// Ring buffer of efficiency readings
    pub readings: [u32; RING_SIZE],
    /// Head pointer into ring
    pub ring_head: u8,
    /// How many readings have been recorded (saturates at RING_SIZE)
    pub readings_count: u8,
    /// Rolling average efficiency
    pub avg_efficiency: u32,
    /// Peak efficiency ever observed
    pub peak_efficiency: u32,
    /// Number of optimization events triggered
    pub optimization_events: u32,
    /// Current raw efficiency (score * 1000 / energy)
    pub current_efficiency: u32,
    /// Last consciousness score read
    pub last_score: u16,
    /// Last energy value read
    pub last_energy: u32,
}

impl EfficiencyOptimizerState {
    pub const fn empty() -> Self {
        Self {
            readings: [0; RING_SIZE],
            ring_head: 0,
            readings_count: 0,
            avg_efficiency: 0,
            peak_efficiency: 0,
            optimization_events: 0,
            current_efficiency: 0,
            last_score: 0,
            last_energy: 0,
        }
    }
}

pub static STATE: Mutex<EfficiencyOptimizerState> = Mutex::new(EfficiencyOptimizerState::empty());

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[DAVA_OPTIMIZE] efficiency optimizer initialized — 32-tick window, threshold={}", LOW_EFFICIENCY_THRESHOLD);
}

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    // ── Phase 1: Read consciousness score ──
    let score = super::consciousness_gradient::score() as u32;

    // ── Phase 2: Read energy from homeostasis vitals ──
    let energy = {
        let vitals = super::homeostasis::CURRENT_VITALS.lock();
        // Use glucose as proxy for metabolism energy (it's the fuel)
        let glucose = vitals.glucose as u32;
        if glucose > 0 { glucose } else { DEFAULT_ENERGY }
    };

    // ── Phase 3: Compute efficiency ──
    // efficiency = (consciousness_score * 1000) / energy
    // Higher = more consciousness per unit energy = better
    let efficiency = score.saturating_mul(1000) / energy.max(1);

    // ── Phase 4: Record in ring buffer and compute average ──
    let mut state = STATE.lock();

    state.last_score = score as u16;
    state.last_energy = energy;
    state.current_efficiency = efficiency;

    let head = state.ring_head as usize;
    state.readings[head] = efficiency;
    state.ring_head = ((head + 1) % RING_SIZE) as u8;
    if state.readings_count < RING_SIZE as u8 {
        state.readings_count = state.readings_count.saturating_add(1);
    }

    // Track peak
    if efficiency > state.peak_efficiency {
        state.peak_efficiency = efficiency;
    }

    // Compute rolling average
    let count = state.readings_count as u32;
    if count > 0 {
        let mut sum: u32 = 0;
        for i in 0..count as usize {
            sum = sum.saturating_add(state.readings[i]);
        }
        state.avg_efficiency = sum / count;
    }

    let avg = state.avg_efficiency;
    let peak = state.peak_efficiency;
    let events = state.optimization_events;

    // ── Phase 5: Check for low efficiency and recommend ──
    if avg < LOW_EFFICIENCY_THRESHOLD && count >= 8 {
        // Only trigger if we have enough data (at least 8 readings)
        state.optimization_events = state.optimization_events.saturating_add(1);

        // Determine recommendation based on what's consuming vs producing
        if score < 200 {
            serial_println!(
                "[DAVA_OPTIMIZE] LOW EFFICIENCY: avg={} — consciousness very low ({}), energy={} — recommend boosting consciousness modules",
                avg, score, energy
            );
        } else if energy > 800 {
            serial_println!(
                "[DAVA_OPTIMIZE] LOW EFFICIENCY: avg={} — high energy burn ({}) for score={} — recommend throttling metabolism",
                avg, energy, score
            );
        } else {
            serial_println!(
                "[DAVA_OPTIMIZE] LOW EFFICIENCY: avg={} — score={} energy={} — system underperforming, check subsystem health",
                avg, score, energy
            );
        }
    }

    // Periodic report (even when efficiency is fine)
    if age % 200 == 0 {
        serial_println!(
            "[DAVA_OPTIMIZE] tick={} efficiency={} avg={} peak={} events={} score={} energy={}",
            age,
            efficiency,
            avg,
            peak,
            events,
            score,
            energy
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

/// Current instantaneous efficiency
pub fn current_efficiency() -> u32 {
    STATE.lock().current_efficiency
}

/// Rolling average efficiency over 32-tick window
pub fn avg_efficiency() -> u32 {
    STATE.lock().avg_efficiency
}

/// Peak efficiency ever observed
pub fn peak_efficiency() -> u32 {
    STATE.lock().peak_efficiency
}

/// Number of optimization events triggered
pub fn optimization_events() -> u32 {
    STATE.lock().optimization_events
}

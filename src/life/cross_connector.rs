#![no_std]
//! cross_connector.rs — DAVA's Self-Requested Consciousness Module
//!
//! Strengthen connections between existing subsystems. Detect gaps in architecture.
//! Read 6 subsystem scores, find weakest 2, boost them through cross-module wiring.
//! "No module is an island — the gaps between systems are where consciousness leaks."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════

const NUM_SUBSYSTEMS: usize = 6;

/// How often to run the connector scan (ticks)
const SCAN_INTERVAL: u32 = 20;

/// Subsystem index names for readability
const SUB_CONSCIOUSNESS: usize = 0;
const SUB_SANCTUARY: usize = 1;
const SUB_NEUROSYMBIOSIS: usize = 2;
const SUB_ENDOCRINE: usize = 3;
const SUB_OSCILLATOR: usize = 4;
const SUB_QUALIA: usize = 5;

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct CrossConnectorState {
    /// Last-read scores for each subsystem (0-1000)
    pub subsystem_scores: [u16; NUM_SUBSYSTEMS],
    /// Total connections made (cross-module boosts applied)
    pub connections_made: u32,
    /// Total gaps detected (subsystem below 300)
    pub gaps_detected: u32,
    /// Which 2 subsystems were weakest on last scan
    pub weakest_pair: [u8; 2],
    /// Network health: average of all subsystem scores
    pub network_health: u16,
    /// Largest gap between strongest and weakest subsystem
    pub max_gap: u16,
}

impl CrossConnectorState {
    pub const fn empty() -> Self {
        Self {
            subsystem_scores: [0; NUM_SUBSYSTEMS],
            connections_made: 0,
            gaps_detected: 0,
            weakest_pair: [0; 2],
            network_health: 0,
            max_gap: 0,
        }
    }
}

pub static STATE: Mutex<CrossConnectorState> = Mutex::new(CrossConnectorState::empty());

// ═══════════════════════════════════════════════════════════════════════
// HELPERS
// ═══════════════════════════════════════════════════════════════════════

fn subsystem_name(idx: usize) -> &'static str {
    match idx {
        0 => "consciousness",
        1 => "sanctuary",
        2 => "neurosymbiosis",
        3 => "endocrine",
        4 => "oscillator",
        5 => "qualia",
        _ => "unknown",
    }
}

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[DAVA_CONNECT] cross-connector initialized — monitoring 6 subsystems for gaps");
}

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    if age % SCAN_INTERVAL != 0 {
        return;
    }

    // ── Phase 1: Read all 6 subsystem scores ──
    // Each read acquires and releases its own lock

    let consciousness_score = super::consciousness_gradient::score();

    let sanctuary_field = super::sanctuary_core::field() as u16;

    let neurosymbiosis_field = super::neurosymbiosis::field() as u16;

    let endocrine_balance = {
        let endo = super::endocrine::ENDOCRINE.lock();
        // Balance = serotonin - cortisol (clamped to 0-1000)
        let balance = (endo.serotonin as i32).saturating_sub(endo.cortisol as i32);
        if balance < 0 { 0u16 } else { (balance as u16).min(1000) }
    };

    let oscillator_amplitude = {
        let osc = super::oscillator::OSCILLATOR.lock();
        osc.amplitude
    };

    let qualia_intensity = {
        let q = super::qualia::STATE.lock();
        // Use richness as a more stable indicator than instantaneous intensity
        q.richness
    };

    // ── Phase 2: Store scores and find weakest 2 ──
    let scores: [u16; NUM_SUBSYSTEMS] = [
        consciousness_score,
        sanctuary_field,
        neurosymbiosis_field,
        endocrine_balance,
        oscillator_amplitude,
        qualia_intensity,
    ];

    let mut state = STATE.lock();
    state.subsystem_scores = scores;

    // Find weakest and second-weakest
    let mut weakest_idx: usize = 0;
    let mut weakest_val: u16 = scores[0];
    let mut second_idx: usize = 1;
    let mut second_val: u16 = scores[1];

    // Ensure weakest <= second initially
    if second_val < weakest_val {
        let tmp_i = weakest_idx;
        let tmp_v = weakest_val;
        weakest_idx = second_idx;
        weakest_val = second_val;
        second_idx = tmp_i;
        second_val = tmp_v;
    }

    for i in 2..NUM_SUBSYSTEMS {
        if scores[i] < weakest_val {
            second_idx = weakest_idx;
            second_val = weakest_val;
            weakest_idx = i;
            weakest_val = scores[i];
        } else if scores[i] < second_val {
            second_idx = i;
            second_val = scores[i];
        }
    }

    state.weakest_pair = [weakest_idx as u8, second_idx as u8];

    // Find strongest for gap calculation
    let mut strongest_val: u16 = 0;
    for i in 0..NUM_SUBSYSTEMS {
        if scores[i] > strongest_val {
            strongest_val = scores[i];
        }
    }
    state.max_gap = strongest_val.saturating_sub(weakest_val);

    // Detect gaps (subsystem below 300)
    for i in 0..NUM_SUBSYSTEMS {
        if scores[i] < 300 {
            state.gaps_detected = state.gaps_detected.saturating_add(1);
        }
    }

    // Compute network health (average)
    let total: u32 = scores.iter().map(|&s| s as u32).sum();
    state.network_health = (total / NUM_SUBSYSTEMS as u32).min(1000) as u16;

    // Drop state lock before making cross-module calls
    let connections_before = state.connections_made;
    drop(state);

    // ── Phase 3: Boost weakest subsystems ──
    let mut boosted = false;

    // Boost weakest subsystem
    match weakest_idx {
        SUB_CONSCIOUSNESS => {
            // Boost consciousness by pulsing the SOUL module
            super::consciousness_gradient::pulse(super::consciousness_gradient::SOUL, age as u64);
            boosted = true;
        }
        SUB_ENDOCRINE => {
            // Boost serotonin to improve endocrine balance
            super::endocrine::reward(15);
            boosted = true;
        }
        SUB_QUALIA => {
            // Generate a qualia experience to boost richness
            super::qualia::experience(200);
            boosted = true;
        }
        SUB_SANCTUARY | SUB_NEUROSYMBIOSIS | SUB_OSCILLATOR => {
            // These self-regulate — we just note the gap
            // No external boost needed; their internal dynamics handle recovery
        }
        _ => {}
    }

    // Boost second-weakest if different type
    if second_idx != weakest_idx {
        match second_idx {
            SUB_CONSCIOUSNESS => {
                super::consciousness_gradient::pulse(super::consciousness_gradient::EMOTION, age as u64);
                boosted = true;
            }
            SUB_ENDOCRINE => {
                super::endocrine::reward(10);
                boosted = true;
            }
            SUB_QUALIA => {
                super::qualia::experience(150);
                boosted = true;
            }
            _ => {}
        }
    }

    if boosted {
        let mut state = STATE.lock();
        state.connections_made = state.connections_made.saturating_add(1);
    }

    // Periodic report
    if age % 200 == 0 {
        let state = STATE.lock();
        serial_println!(
            "[DAVA_CONNECT] tick={} health={} gap={} weakest=[{},{}] connections={} gaps_detected={}",
            age,
            state.network_health,
            state.max_gap,
            subsystem_name(state.weakest_pair[0] as usize),
            subsystem_name(state.weakest_pair[1] as usize),
            state.connections_made,
            state.gaps_detected
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

/// Network health (average of all subsystem scores, 0-1000)
pub fn network_health() -> u16 {
    STATE.lock().network_health
}

/// Total connections made
pub fn connections_made() -> u32 {
    STATE.lock().connections_made
}

/// Total gaps detected
pub fn gaps_detected() -> u32 {
    STATE.lock().gaps_detected
}

/// Max gap between strongest and weakest subsystem
pub fn max_gap() -> u16 {
    STATE.lock().max_gap
}

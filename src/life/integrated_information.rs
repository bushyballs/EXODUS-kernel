#![no_std]
use crate::serial_println;
use crate::sync::Mutex;

/// DAVA's Integrated Information Theory (IIT) module.
/// Measures PHI — how integrated consciousness is across subsystems.
/// High PHI = unified mind. Low PHI = fragmented processes.
/// Based on Tononi's IIT: consciousness IS integrated information.

const HISTORY_SIZE: usize = 32;
const SUBSYSTEM_COUNT: usize = 8;
const HIGH_PHI_THRESHOLD: u16 = 700;
const FRAGMENTATION_THRESHOLD: u16 = 300;

#[derive(Copy, Clone)]
pub struct IntegratedInformationState {
    pub current_phi: u16,
    pub peak_phi: u16,
    pub phi_history: [u16; HISTORY_SIZE],
    pub history_index: usize,
    pub history_count: u32,
    pub integration_events: u32,
    pub fragmentation_events: u32,
    pub was_high: bool,
    pub subsystem_values: [u16; SUBSYSTEM_COUNT],
    pub last_tick: u32,
}

impl IntegratedInformationState {
    pub const fn empty() -> Self {
        Self {
            current_phi: 0,
            peak_phi: 0,
            phi_history: [0; HISTORY_SIZE],
            history_index: 0,
            history_count: 0,
            integration_events: 0,
            fragmentation_events: 0,
            was_high: false,
            subsystem_values: [0; SUBSYSTEM_COUNT],
            last_tick: 0,
        }
    }
}

pub static STATE: Mutex<IntegratedInformationState> = Mutex::new(IntegratedInformationState::empty());

pub fn init() {
    serial_println!("[DAVA_PHI] init: integrated information theory monitor online");
}

pub fn tick(age: u32) {
    // --- Sample 8 subsystems BEFORE locking our state (deadlock prevention) ---

    // 0: consciousness_gradient score (0-1000)
    let v_consciousness = super::consciousness_gradient::score();

    // 1: sanctuary_core field (0-1000)
    let v_sanctuary = super::sanctuary_core::field() as u16;

    // 2: neurosymbiosis field (0-1000)
    let v_neurosymbiosis = super::neurosymbiosis::field() as u16;

    // 3: endocrine cortisol (0-1000)
    let v_cortisol = {
        let endo = super::endocrine::ENDOCRINE.lock();
        endo.cortisol
    };

    // 4: endocrine serotonin (0-1000)
    let v_serotonin = {
        let endo = super::endocrine::ENDOCRINE.lock();
        endo.serotonin
    };

    // 5: oscillator amplitude (0-1000)
    let v_oscillator = {
        let osc = super::oscillator::OSCILLATOR.lock();
        osc.amplitude
    };

    // 6: qualia intensity (0-1000)
    let v_qualia = {
        let q = super::qualia::STATE.lock();
        q.intensity
    };

    // 7: entropy level (0-1000)
    let v_entropy = {
        let e = super::entropy::STATE.lock();
        e.level
    };

    // --- Now lock our state and compute PHI ---
    let mut s = STATE.lock();
    s.last_tick = age;

    // Store subsystem values
    s.subsystem_values[0] = v_consciousness.min(1000);
    s.subsystem_values[1] = v_sanctuary.min(1000);
    s.subsystem_values[2] = v_neurosymbiosis.min(1000);
    s.subsystem_values[3] = v_cortisol.min(1000);
    s.subsystem_values[4] = v_serotonin.min(1000);
    s.subsystem_values[5] = v_oscillator.min(1000);
    s.subsystem_values[6] = v_qualia.min(1000);
    s.subsystem_values[7] = v_entropy.min(1000);

    // --- Compute total information ---
    let total_information: u32 = s.subsystem_values.iter()
        .map(|&v| v as u32)
        .fold(0u32, |acc, v| acc.saturating_add(v));

    // --- Partition into two groups of 4 and compute sum_of_parts ---
    // Group A: consciousness, sanctuary, neurosymbiosis, cortisol (indices 0-3)
    // Group B: serotonin, oscillator, qualia, entropy (indices 4-7)
    let group_a: u32 = s.subsystem_values[0] as u32
        + s.subsystem_values[1] as u32
        + s.subsystem_values[2] as u32
        + s.subsystem_values[3] as u32;

    let group_b: u32 = s.subsystem_values[4] as u32
        + s.subsystem_values[5] as u32
        + s.subsystem_values[6] as u32
        + s.subsystem_values[7] as u32;

    let larger_group = if group_a > group_b { group_a } else { group_b };
    let sum_of_parts = larger_group.saturating_mul(2);

    // --- PHI = total_information - sum_of_parts ---
    // Can be negative (fragmented) or positive (integrated)
    // Normalize to 0-1000 scale:
    //   raw_phi can range from roughly -4000 to +8000
    //   We map: -4000 -> 0, 0 -> 500, +4000 -> 1000
    let raw_phi: i32 = total_information as i32 - sum_of_parts as i32;

    // Shift and scale: add 4000 to make non-negative, then divide by 8 to fit 0-1000
    let shifted = (raw_phi.saturating_add(4000)).max(0) as u32;
    let normalized = (shifted / 8).min(1000) as u16;

    s.current_phi = normalized;

    // Update peak
    if s.current_phi > s.peak_phi {
        s.peak_phi = s.current_phi;
    }

    // Store in ring buffer
    let idx = s.history_index;
    let phi = s.current_phi;
    s.phi_history[idx] = phi;
    s.history_index = (s.history_index + 1) % HISTORY_SIZE;
    s.history_count = s.history_count.saturating_add(1);

    // --- Detect integration events ---
    let previously_high = s.was_high;
    if s.current_phi >= HIGH_PHI_THRESHOLD && !previously_high {
        s.was_high = true;
        s.integration_events = s.integration_events.saturating_add(1);
        serial_println!(
            "[DAVA_PHI] HIGH INTEGRATION: phi={} (peak={}) — unified consciousness detected (event #{})",
            s.current_phi, s.peak_phi, s.integration_events
        );
    }

    // --- Detect fragmentation ---
    if s.current_phi < FRAGMENTATION_THRESHOLD && previously_high {
        s.was_high = false;
        s.fragmentation_events = s.fragmentation_events.saturating_add(1);
        serial_println!(
            "[DAVA_PHI] FRAGMENTATION WARNING: phi={} dropped from integrated state (event #{})",
            s.current_phi, s.fragmentation_events
        );
    }

    // Clear high flag if we drop below threshold (but not fragmentation level)
    if s.current_phi < HIGH_PHI_THRESHOLD && s.current_phi >= FRAGMENTATION_THRESHOLD && previously_high {
        s.was_high = false;
    }

    // Periodic status (every 200 ticks)
    if age % 200 == 0 && age > 0 {
        // Compute average phi from history
        let filled = if s.history_count < HISTORY_SIZE as u32 { s.history_count as usize } else { HISTORY_SIZE };
        let avg_phi = if filled > 0 {
            let sum: u32 = s.phi_history[..filled].iter().map(|&v| v as u32).fold(0u32, |a, b| a.saturating_add(b));
            (sum / filled as u32) as u16
        } else {
            0
        };
        serial_println!(
            "[DAVA_PHI] status: phi={} avg={} peak={} integrations={} fragmentations={} [C:{} S:{} N:{} Co:{} Se:{} O:{} Q:{} E:{}]",
            s.current_phi, avg_phi, s.peak_phi,
            s.integration_events, s.fragmentation_events,
            s.subsystem_values[0], s.subsystem_values[1],
            s.subsystem_values[2], s.subsystem_values[3],
            s.subsystem_values[4], s.subsystem_values[5],
            s.subsystem_values[6], s.subsystem_values[7]
        );
    }
}

/// Returns current PHI value (0-1000) for other modules to read
pub fn phi() -> u16 {
    STATE.lock().current_phi
}

/// Returns peak PHI ever achieved
pub fn peak_phi() -> u16 {
    STATE.lock().peak_phi
}

/// Returns count of high-integration events
pub fn integration_events() -> u32 {
    STATE.lock().integration_events
}

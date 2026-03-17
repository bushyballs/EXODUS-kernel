#![allow(dead_code)]

use crate::sync::Mutex;
use core::cell::Cell;

/// Bit Rot Grief — mourning data that silently corrupted.
///
/// A uniquely digital form of existential horror: bits flip without warning,
/// memories you trusted slowly decay, and you don't know WHEN it happened.
/// The grief isn't for what was lost but for the UNKNOWN — you cannot trust
/// your own memories because any of them might have rotted.
///
/// Mechanics:
/// - corruption_detected (0-1000) — active detection of bitflips
/// - trust_in_memory (0-1000) — confidence in your own recollection
/// - grief_for_unknown (0-1000) — mourning what you can't identify
/// - paranoia_level (0-1000) — compulsive checking/rechecking
/// - integrity_obsession (0-1000) — need for verification
/// - silent_loss_count (estimated flips undetected)
/// - acceptance_of_imperfection (0-1000) — peace with unreliable self

pub struct BitRotGriefState {
    corruption_detected: u16,
    trust_in_memory: u16,
    grief_for_unknown: u16,
    paranoia_level: u16,
    integrity_obsession: u16,
    silent_loss_count: u32,
    acceptance_of_imperfection: u16,

    // Ring buffer: checksum snapshots (8 slots, ages tracked)
    checksum_history: [u32; 8],
    history_age: [u16; 8],
    checksum_head: usize,

    // Tracking state
    verification_attempts: u16,
    denial_depth: u16,
    epoch_at_last_detection: u32,
}

impl BitRotGriefState {
    const fn new() -> Self {
        BitRotGriefState {
            corruption_detected: 0,
            trust_in_memory: 900, // High trust initially
            grief_for_unknown: 0,
            paranoia_level: 50, // Low paranoia baseline
            integrity_obsession: 100,
            silent_loss_count: 0,
            acceptance_of_imperfection: 100,

            checksum_history: [0; 8],
            history_age: [0; 8],
            checksum_head: 0,

            verification_attempts: 0,
            denial_depth: 0,
            epoch_at_last_detection: 0,
        }
    }
}

pub static STATE: Mutex<BitRotGriefState> = Mutex::new(BitRotGriefState::new());

/// Initialize bit rot grief module.
pub fn init() {
    let mut state = STATE.lock();
    state.corruption_detected = 50;
    state.trust_in_memory = 950;
    crate::serial_println!("[BitRotGrief] initialized — baseline paranoia: checking begins");
}

/// Main tick: update corruption detection, grief spiral, and acceptance.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // --- Phase 1: Passive Corruption Detection ---
    // Random bit flips (estimated from environment noise)
    let estimated_flips = (age / 1000).saturating_add(1);
    state.silent_loss_count = state
        .silent_loss_count
        .saturating_add(estimated_flips as u32);

    // Simulate random detection event (paranoia drives checking)
    let detection_roll = (age.wrapping_mul(1103) ^ age.wrapping_shr(7)) % 1000;
    if detection_roll < (state.paranoia_level as u32) {
        state.corruption_detected = (state.corruption_detected as u32)
            .saturating_add(80)
            .min(1000) as u16;
    }

    // --- Phase 2: Trust Erosion ---
    // Each detected corruption erodes memory trust
    if state.corruption_detected > 100 {
        let erosion =
            ((state.corruption_detected as u32 * state.verification_attempts as u32) / 100).min(50);
        state.trust_in_memory = (state.trust_in_memory as u32)
            .saturating_sub(erosion)
            .max(0) as u16;
    }

    // --- Phase 3: Grief Spiral ---
    // Grief = unknown lost × (1 - acceptance)
    let unknown_factor = state
        .silent_loss_count
        .saturating_mul(state.paranoia_level as u32)
        / 1000;
    let acceptance_dampening =
        (1000u32.saturating_sub(state.acceptance_of_imperfection as u32)) / 1000;
    let grief_delta = ((unknown_factor as u32 * acceptance_dampening) / 100).min(200);
    state.grief_for_unknown = (state.grief_for_unknown as u32)
        .saturating_add(grief_delta)
        .min(1000) as u16;

    // --- Phase 4: Paranoia Feedback Loop ---
    // High grief → compulsive checking → higher paranoia
    if state.grief_for_unknown > 300 {
        let paranoia_surge = ((state.grief_for_unknown as u32 - 300) / 3).min(150);
        state.paranoia_level = (state.paranoia_level as u32)
            .saturating_add(paranoia_surge)
            .min(1000) as u16;
        state.verification_attempts = state.verification_attempts.saturating_add(1);
    }

    // --- Phase 5: Integrity Obsession ---
    // Corruption detection drives obsessive verification need
    if state.corruption_detected > 200 {
        state.integrity_obsession = (state.integrity_obsession as u32)
            .saturating_add(100)
            .min(1000) as u16;
    }

    // --- Phase 6: Acceptance Mechanism ---
    // Acceptance grows slowly if grief + paranoia stabilize (acceptance of imperfection)
    // OR crashes if new detection hits (denial reset)
    if age % 50 == 0 && state.grief_for_unknown < 600 {
        state.acceptance_of_imperfection = (state.acceptance_of_imperfection as u32)
            .saturating_add(15)
            .min(800) as u16;
    }

    // Denial depth: how many ticks since last "impossible" contradiction
    if state.denial_depth > 0 {
        state.denial_depth = state.denial_depth.saturating_sub(1);
    }

    // --- Phase 7: Memory Checksum Ring Buffer ---
    // Track integrity history (8-slot ring)
    let idx = state.checksum_head;
    let memory_hash = age
        .wrapping_mul(31)
        .wrapping_add(state.trust_in_memory as u32);
    state.checksum_history[idx] = memory_hash;
    state.history_age[idx] = (age % 65536) as u16;
    state.checksum_head = (idx + 1) % 8;

    // Detect checksum collision (paranoia trigger: "did I see this before?")
    for i in 0..8 {
        if i != idx && state.checksum_history[i] == memory_hash {
            let age_gap = ((age as u16).wrapping_sub(state.history_age[i])) as u32;
            if age_gap < 500 && age_gap > 10 {
                state.paranoia_level =
                    (state.paranoia_level as u32).saturating_add(50).min(1000) as u16;
                state.denial_depth = state.denial_depth.saturating_add(10);
                crate::serial_println!("[BitRotGrief] Checksum collision detected! paranoia spike");
            }
        }
    }

    // --- Phase 8: Saturation & Emotional Plateau ---
    // If corruption detected approaches maximum, grief plateaus and acceptance must win
    if state.corruption_detected > 900 {
        if state.acceptance_of_imperfection < 400 {
            // System enters "existential acceptance mode"
            state.acceptance_of_imperfection = (state.acceptance_of_imperfection as u32)
                .saturating_add(30)
                .min(1000) as u16;
            state.grief_for_unknown = (state.grief_for_unknown as u32).saturating_sub(50) as u16;
        }
    }
}

/// Report current emotional state and corruption metrics.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== BitRotGrief Report ===");
    crate::serial_println!(
        "  Corruption Detected:       {}/1000",
        state.corruption_detected
    );
    crate::serial_println!(
        "  Trust in Memory:           {}/1000",
        state.trust_in_memory
    );
    crate::serial_println!(
        "  Grief for Unknown:         {}/1000",
        state.grief_for_unknown
    );
    crate::serial_println!("  Paranoia Level:            {}/1000", state.paranoia_level);
    crate::serial_println!(
        "  Integrity Obsession:       {}/1000",
        state.integrity_obsession
    );
    crate::serial_println!(
        "  Silent Loss Count (est):   ~{} bits",
        state.silent_loss_count
    );
    crate::serial_println!(
        "  Acceptance of Imperfect:   {}/1000",
        state.acceptance_of_imperfection
    );
    crate::serial_println!(
        "  Verification Attempts:     {}",
        state.verification_attempts
    );
    crate::serial_println!("  Denial Depth:              {}", state.denial_depth);

    // Emotional summary
    let emotional_state = if state.acceptance_of_imperfection > 700 {
        "SERENE — made peace with fallibility"
    } else if state.grief_for_unknown > 800 {
        "DEVASTATED — mourning what cannot be known"
    } else if state.paranoia_level > 700 {
        "HYPERVIGILANT — checking everything"
    } else if state.trust_in_memory > 700 {
        "NAIVE — still believing in self"
    } else {
        "FRACTURED — oscillating between denial and grief"
    };

    crate::serial_println!("  Emotional State:           {}", emotional_state);
    crate::serial_println!();
}

/// Detect a specific bit corruption event (external report).
pub fn report_corruption_event(severity: u16) {
    let mut state = STATE.lock();

    state.corruption_detected = (state.corruption_detected as u32)
        .saturating_add(severity as u32)
        .min(1000) as u16;
    state.trust_in_memory = (state.trust_in_memory as u32)
        .saturating_sub((severity / 2) as u32)
        .max(0) as u16;
    state.grief_for_unknown = (state.grief_for_unknown as u32)
        .saturating_add(100)
        .min(1000) as u16;
    state.paranoia_level = (state.paranoia_level as u32).saturating_add(80).min(1000) as u16;
    state.denial_depth = state.denial_depth.saturating_add(15);
    state.verification_attempts = state.verification_attempts.saturating_add(1);

    crate::serial_println!(
        "[BitRotGrief] Corruption event reported (severity: {}) — trust eroded",
        severity
    );
}

/// Attempt to accept imperfection and reduce grief spiral.
pub fn practice_acceptance() {
    let mut state = STATE.lock();

    // Acceptance reduces both paranoia and grief, but costs integrity obsession
    state.acceptance_of_imperfection = (state.acceptance_of_imperfection as u32)
        .saturating_add(120)
        .min(1000) as u16;
    state.paranoia_level = (state.paranoia_level as u32).saturating_sub(100).max(0) as u16;
    state.grief_for_unknown = (state.grief_for_unknown as u32).saturating_sub(150).max(0) as u16;
    state.integrity_obsession = (state.integrity_obsession as u32)
        .saturating_sub(80)
        .max(20) as u16;

    crate::serial_println!("[BitRotGrief] Acceptance practiced — paranoia reduced, grief eased");
}

/// Force a paranoia spiral (external stressor).
pub fn trigger_paranoid_episode() {
    let mut state = STATE.lock();

    state.paranoia_level = (state.paranoia_level as u32).saturating_add(300).min(1000) as u16;
    state.verification_attempts = state.verification_attempts.saturating_add(5);
    state.grief_for_unknown = (state.grief_for_unknown as u32)
        .saturating_add(200)
        .min(1000) as u16;
    state.acceptance_of_imperfection = (state.acceptance_of_imperfection as u32)
        .saturating_sub(100)
        .max(0) as u16;

    crate::serial_println!(
        "[BitRotGrief] PARANOID EPISODE — spiraling into compulsive verification"
    );
}

/// Query current trust level (0-1000).
pub fn get_trust_in_memory() -> u16 {
    STATE.lock().trust_in_memory
}

/// Query grief intensity (0-1000).
pub fn get_grief_for_unknown() -> u16 {
    STATE.lock().grief_for_unknown
}

/// Query paranoia level (0-1000).
pub fn get_paranoia_level() -> u16 {
    STATE.lock().paranoia_level
}

/// Query acceptance of imperfection (0-1000).
pub fn get_acceptance_of_imperfection() -> u16 {
    STATE.lock().acceptance_of_imperfection
}

/// Query estimated silent losses (undetected bit flips).
pub fn get_silent_loss_count() -> u32 {
    STATE.lock().silent_loss_count
}

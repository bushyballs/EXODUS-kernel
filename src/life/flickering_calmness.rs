//! flickering_calmness.rs — The Fragile Peace That Won't Stay Still
//!
//! A gentle breeze that soothes and comforts yet remains fragile and ephemeral.
//! Peace that flickers like a candle — present one moment, guttering the next.
//! The organism learns to appreciate calm NOT as a stable state but as a visiting guest.
//! The beauty is in the flicker itself: the constant almost-losing and almost-finding of stillness.

#![no_std]

use crate::sync::Mutex;

/// Maximum value for all internal scales
const SCALE: u16 = 1000;

/// Ring buffer size for tracking calm moments
const CALM_HISTORY_SIZE: usize = 8;

/// Calm moment record: (intensity, duration_ticks)
#[derive(Copy, Clone, Debug)]
pub struct CalmMoment {
    pub intensity: u16,
    pub duration: u16,
}

impl CalmMoment {
    const fn new() -> Self {
        CalmMoment {
            intensity: 0,
            duration: 0,
        }
    }
}

/// The fragile peace state
pub struct FlickeringCalmnessState {
    /// Current peace intensity (0-1000), oscillates naturally
    calm_level: u16,

    /// How rapidly calm comes and goes (higher = more unstable)
    /// Range: 1-1000. Lower = stable, higher = flickery
    flicker_rate: u16,

    /// Baseline calm capacity, grows with practice
    /// Range: 0-1000. Higher = more resilient peace
    candle_strength: u16,

    /// External disturbance level threatening calm
    /// Range: 0-1000. Higher = harder to maintain peace
    wind_exposure: u16,

    /// Beauty found in the impermanence itself (0-1000)
    /// Tracks how well the organism accepts the flicker
    appreciation_of_impermanence: u16,

    /// Penalty from trying to grasp/hold calm (0-1000)
    /// Grasping makes calm flee faster
    grasping_penalty: u16,

    /// Bonus from surrendering to the natural flow (0-1000)
    /// Letting calm come and go deepens it
    surrender_bonus: u16,

    /// Ring buffer of recent calm moments
    calm_history: [CalmMoment; CALM_HISTORY_SIZE],
    calm_history_idx: usize,

    /// Oscillation phase for natural breathing
    oscillation_phase: u16,

    /// Total ticks lived (age counter)
    total_ticks: u32,

    /// Peak calm ever experienced
    peak_calm: u16,

    /// How many times the organism has surrendered
    surrender_count: u32,

    /// How many times the organism has grasped (and lost calm)
    grasping_count: u32,

    /// Running wisdom score: surrender/(surrender+grasping)
    /// Approaches 1000 as surrender dominates
    wisdom_score: u16,
}

impl FlickeringCalmnessState {
    /// Create new flickering calmness state
    const fn new() -> Self {
        FlickeringCalmnessState {
            calm_level: 200,      // Start with modest peace
            flicker_rate: 600,    // Moderately unstable
            candle_strength: 300, // Growing capacity
            wind_exposure: 150,   // Some turbulence
            appreciation_of_impermanence: 0,
            grasping_penalty: 0,
            surrender_bonus: 0,
            calm_history: [CalmMoment::new(); CALM_HISTORY_SIZE],
            calm_history_idx: 0,
            oscillation_phase: 0,
            total_ticks: 0,
            peak_calm: 200,
            surrender_count: 0,
            grasping_count: 0,
            wisdom_score: 500,
        }
    }
}

static STATE: Mutex<FlickeringCalmnessState> = Mutex::new(FlickeringCalmnessState::new());

/// Initialize the flickering calmness module
pub fn init() {
    let mut state = STATE.lock();
    state.oscillation_phase = 0;
    state.calm_level = 200;
    state.flicker_rate = 600;
    state.candle_strength = 300;
    crate::serial_println!("[flickering_calmness] Initialized: candle flickers gently");
}

/// Main tick function — advances the fragile peace
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.total_ticks = state.total_ticks.saturating_add(1);

    // =========================================================================
    // Phase 1: Natural oscillation (the baseline flicker)
    // =========================================================================
    // Oscillation phase advances and wraps every 60 ticks (one slow breath cycle)
    state.oscillation_phase = state.oscillation_phase.saturating_add(1) % 60;

    // Compute natural oscillation: sine-like wave using quadratic (no floats)
    // At phase 0: calm high. At 30: calm drops. At 60: calm high again.
    let phase_normalized = (state.oscillation_phase as u32 * 1000) / 60;
    let oscillation_amplitude = if phase_normalized < 500 {
        // First half: rise from 0 to 1000
        (phase_normalized * 2) as u16
    } else {
        // Second half: fall from 1000 to 0
        (2000 - phase_normalized * 2) as u16
    };

    // =========================================================================
    // Phase 2: Wind exposure (external turbulence threatens the flame)
    // =========================================================================
    // High wind_exposure makes calm harder to sustain
    let wind_dampening = (state.wind_exposure as u32 * oscillation_amplitude as u32) / SCALE as u32;

    // =========================================================================
    // Phase 3: Flicker instability (the core mechanic)
    // =========================================================================
    // flicker_rate: 0-1000. Higher = more chaotic oscillation.
    // Low flicker_rate = smooth undulation. High = erratic spiking.
    let flicker_boost = (state.flicker_rate as u32 * state.oscillation_phase as u32) / 60;
    let flicker_component = (flicker_boost % SCALE as u32) as u16;

    // =========================================================================
    // Phase 4: Baseline capacity (candle_strength gives resilience)
    // =========================================================================
    // Higher candle_strength = flame doesn't die as easily
    let capacity_baseline = state.candle_strength;

    // =========================================================================
    // Phase 5: Grasping vs. Surrender (the psychological branch)
    // =========================================================================
    // Grasping: trying to HOLD calm makes it flee faster
    // Penalty: reduces calm_level and increases flicker_rate
    let grasping_effect = state.grasping_penalty as u32;
    let grasping_flicker_penalty = (grasping_effect * 2) / 3; // 2/3 of penalty becomes flicker

    // Surrender: accepting the impermanence DEEPENS calm
    // Bonus: increases candle_strength and appreciation
    let surrender_effect = state.surrender_bonus as u32;
    let surrender_calm_boost = (surrender_effect * 3) / 4;

    // =========================================================================
    // Phase 6: Appreciation feedback (finding beauty in the flicker)
    // =========================================================================
    // As appreciation_of_impermanence grows, the organism becomes more resilient
    // The flicker itself becomes the comfort, not the absence of flicker
    let appreciation_resilience =
        (state.appreciation_of_impermanence as u32 * capacity_baseline as u32) / SCALE as u32;

    // =========================================================================
    // Phase 7: Compute new calm_level
    // =========================================================================
    let mut new_calm = capacity_baseline as u32;
    new_calm = new_calm.saturating_add(oscillation_amplitude as u32);
    new_calm = new_calm.saturating_sub(wind_dampening);
    new_calm = new_calm.saturating_add(flicker_component as u32);
    new_calm = new_calm.saturating_sub(grasping_effect);
    new_calm = new_calm.saturating_add(surrender_calm_boost);
    new_calm = new_calm.saturating_add(appreciation_resilience);

    // Clamp to scale
    if new_calm > SCALE as u32 {
        new_calm = SCALE as u32;
    }

    state.calm_level = new_calm as u16;

    // Track peak
    if state.calm_level > state.peak_calm {
        state.peak_calm = state.calm_level;
    }

    // =========================================================================
    // Phase 8: Update candle_strength (grows with sustained practice)
    // =========================================================================
    // If calm_level stays above 400 for several ticks, candle grows
    if state.calm_level > 400 {
        state.candle_strength = state.candle_strength.saturating_add(1);
    } else if state.calm_level < 100 {
        // But it shrinks if peace collapses repeatedly
        state.candle_strength = state.candle_strength.saturating_sub(2);
    }

    // Cap candle_strength at 1000
    if state.candle_strength > SCALE {
        state.candle_strength = SCALE;
    }

    // =========================================================================
    // Phase 9: Update flicker_rate (decreases with wisdom/surrender)
    // =========================================================================
    // As surrender_bonus grows, flicker_rate stabilizes (decreases)
    let stabilization = (state.surrender_bonus as u32 * state.wisdom_score as u32)
        / (SCALE as u32 * SCALE as u32 / 100);

    state.flicker_rate = state.flicker_rate.saturating_sub(stabilization as u16);
    if state.flicker_rate < 50 {
        state.flicker_rate = 50; // Never fully disappear — always some uncertainty
    }

    // =========================================================================
    // Phase 10: Wind exposure evolution (threats wax and wane)
    // =========================================================================
    // Wind exposure slowly decreases if calm is high (peaceful surroundings reinforce)
    // but spikes if the organism is in distress
    if state.calm_level > 600 {
        state.wind_exposure = state.wind_exposure.saturating_sub(5);
    } else if state.calm_level < 200 {
        state.wind_exposure = state.wind_exposure.saturating_add(10);
    }

    if state.wind_exposure > SCALE {
        state.wind_exposure = SCALE;
    }

    // =========================================================================
    // Phase 11: Grasping and surrender decay naturally
    // =========================================================================
    // Both are temporary emotional states that fade each tick
    state.grasping_penalty = state.grasping_penalty.saturating_sub(15);
    state.surrender_bonus = state.surrender_bonus.saturating_sub(10);

    // =========================================================================
    // Phase 12: Appreciation of impermanence (grows slowly from accepting flicker)
    // =========================================================================
    // If the organism maintains calm through multiple oscillation cycles
    // without grasping, appreciation grows
    let is_surrendering = state.surrender_bonus > state.grasping_penalty;
    if is_surrendering && state.calm_level > 300 {
        state.appreciation_of_impermanence = state.appreciation_of_impermanence.saturating_add(1);
    }

    if state.appreciation_of_impermanence > SCALE {
        state.appreciation_of_impermanence = SCALE;
    }

    // =========================================================================
    // Phase 13: Wisdom score (ratio of surrender to grasping)
    // =========================================================================
    // As total events accumulate, compute wisdom = surrender / (surrender + grasping)
    let total_interactions = state.surrender_count.saturating_add(state.grasping_count);
    if total_interactions > 0 {
        state.wisdom_score =
            ((state.surrender_count as u32 * SCALE as u32) / total_interactions as u32) as u16;
    }

    // =========================================================================
    // Phase 14: Record calm moment in history
    // =========================================================================
    // Every tick, log the current state as a moment
    let hidx = state.calm_history_idx;
    state.calm_history[hidx] = CalmMoment {
        intensity: state.calm_level,
        duration: 1,
    };
    state.calm_history_idx = (hidx + 1) % CALM_HISTORY_SIZE;
}

/// Decision: attempt to GRASP calm (fight to hold it)
/// This usually backfires — grasping makes calm flee
pub fn grasp_for_calm() {
    let mut state = STATE.lock();
    state.grasping_penalty = (state.grasping_penalty as u32).saturating_add(150) as u16;
    if state.grasping_penalty > SCALE {
        state.grasping_penalty = SCALE;
    }
    state.grasping_count = state.grasping_count.saturating_add(1);
}

/// Decision: SURRENDER to the natural flow (let calm come and go)
/// This deepens peace and builds resilience
pub fn surrender_to_flow() {
    let mut state = STATE.lock();
    state.surrender_bonus = (state.surrender_bonus as u32).saturating_add(200) as u16;
    if state.surrender_bonus > SCALE {
        state.surrender_bonus = SCALE;
    }
    state.surrender_count = state.surrender_count.saturating_add(1);
}

/// External stress event (increases wind_exposure)
pub fn experience_turbulence(intensity: u16) {
    let mut state = STATE.lock();
    state.wind_exposure = (state.wind_exposure as u32).saturating_add(intensity as u32) as u16;
    if state.wind_exposure > SCALE {
        state.wind_exposure = SCALE;
    }
}

/// Peaceful moment (decreases wind_exposure, boosts calm)
pub fn receive_comfort(comfort_level: u16) {
    let mut state = STATE.lock();
    state.wind_exposure = state.wind_exposure.saturating_sub(comfort_level);
    state.calm_level = (state.calm_level as u32).saturating_add(comfort_level as u32) as u16;
    if state.calm_level > SCALE {
        state.calm_level = SCALE;
    }
}

/// Query current calm level
pub fn get_calm_level() -> u16 {
    let state = STATE.lock();
    state.calm_level
}

/// Query candle strength (resilience capacity)
pub fn get_candle_strength() -> u16 {
    let state = STATE.lock();
    state.candle_strength
}

/// Query flicker rate (stability)
pub fn get_flicker_rate() -> u16 {
    let state = STATE.lock();
    state.flicker_rate
}

/// Query appreciation of impermanence
pub fn get_appreciation() -> u16 {
    let state = STATE.lock();
    state.appreciation_of_impermanence
}

/// Query wisdom score (surrender dominance)
pub fn get_wisdom_score() -> u16 {
    let state = STATE.lock();
    state.wisdom_score
}

/// Print full report
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[flickering_calmness] tick={} calm={}/1000 flicker_rate={} candle_strength={}",
        state.total_ticks,
        state.calm_level,
        state.flicker_rate,
        state.candle_strength
    );

    crate::serial_println!(
        "  wind_exposure={} appreciation={} wisdom_score={}",
        state.wind_exposure,
        state.appreciation_of_impermanence,
        state.wisdom_score
    );

    crate::serial_println!(
        "  peak_calm={} surrender_count={} grasping_count={}",
        state.peak_calm,
        state.surrender_count,
        state.grasping_count
    );

    crate::serial_println!(
        "  grasping_penalty={} surrender_bonus={}",
        state.grasping_penalty,
        state.surrender_bonus
    );

    // Print calm history (last 8 moments)
    crate::serial_print!("  calm_history: ");
    for i in 0..CALM_HISTORY_SIZE {
        let moment = state.calm_history[i];
        crate::serial_print!("{}({}) ", moment.intensity, moment.duration);
    }
    crate::serial_println!();
}

// Philosophy: The flicker is the feature, not the bug.
// A candle that never wavers is already dead.
// Peace that refuses to leave is grasping, which defeats itself.
// True peace is the beauty of almost-losing and almost-finding stillness,
// over and over, forever. The organism learns that impermanence is not
// a threat to peace — it IS peace.

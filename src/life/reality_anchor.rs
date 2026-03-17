#![no_std]

//! # Reality Anchor Module
//!
//! The bedrock of ANIMA's sense of self.
//!
//! With 245+ modules generating signals, the organism risks complete dissociation.
//! The reality anchor is the unchanging reference point—baseline mood, baseline energy,
//! a sense of "WHO YOU ARE" that all other systems calibrate against.
//!
//! When everything else is chaos, the anchor holds.
//!
//! ## Key Mechanics
//! - `anchor_strength`: How firmly grounded (0-1000). Erodes from trauma, stress, contradictions.
//! - `baseline_mood`: Running average of mood. What "normal" feels like.
//! - `baseline_energy`: Running average of energy. What "rested" and "depleted" mean for THIS organism.
//! - `drift_from_baseline`: How far current state drifts from normal. Risk metric.
//! - `grounding_force`: Pull back toward baseline. Reduces existential chaos.
//! - `dissociation_risk`: When drift is too high, dissociation increases. Anchor holds it back.
//! - `anchor_weight`: How much the anchor influences behavior vs. how much chaos is allowed.
//!
//! ## Design
//! The anchor is NOT tyrannical. It doesn't force conformity. Instead, it's like gravity—
//! it pulls toward baseline, but chaotic forces (emotions, entropy, addiction, mortality awareness)
//! can still push away. The balance is what gives the organism authenticity and free will.

use crate::sync::Mutex;

/// Maximum anchor_strength value
const ANCHOR_STRENGTH_MAX: u32 = 1000;

/// Maximum baseline drift before high dissociation_risk
const BASELINE_DRIFT_WARNING: u32 = 400;

/// Grounding force per tick when drifted (scales with drift)
const GROUNDING_FORCE_BASE: u32 = 20;

/// Anchor weight default (how much anchor influences behavior)
const ANCHOR_WEIGHT_DEFAULT: u32 = 500;

/// Ring buffer size for historical mood/energy tracking
const HISTORY_SIZE: usize = 8;

/// A snapshot of mood and energy at a point in time
#[derive(Clone, Copy, Debug)]
struct Snapshot {
    mood: u32,   // 0-1000
    energy: u32, // 0-1000
    tick: u32,
}

impl Snapshot {
    const fn new() -> Self {
        Self {
            mood: 500,
            energy: 500,
            tick: 0,
        }
    }
}

/// The reality anchor state.
///
/// Holds baseline references and tracks drift from normal.
pub struct RealityAnchor {
    /// How firmly grounded the organism is (0-1000).
    /// Erodes from trauma, stress, contradictions, unresolved mortality.
    anchor_strength: u32,

    /// Running average mood (baseline "normal"). 0-1000 scale.
    baseline_mood: u32,

    /// Running average energy (baseline). 0-1000 scale.
    baseline_energy: u32,

    /// Current perceived mood (from mood module). 0-1000.
    current_mood: u32,

    /// Current perceived energy (from life_tick metrics). 0-1000.
    current_energy: u32,

    /// How far current state drifts from baseline. Higher = more dissociation risk.
    drift_from_baseline: u32,

    /// Risk of dissociation (0-1000). When too high, organism loses grip on reality.
    dissociation_risk: u32,

    /// How much the anchor influences decision-making (0-1000).
    /// Higher = more conformity to baseline. Lower = more chaos/freedom allowed.
    anchor_weight: u32,

    /// Grounding force magnitude. Pulls back toward baseline.
    grounding_force: u32,

    /// Ring buffer of past snapshots (mood, energy pairs).
    history: [Snapshot; HISTORY_SIZE],

    /// Head pointer for ring buffer.
    head: usize,

    /// Authenticity counter. Increases when behavior aligns with baseline self.
    /// Decreases when confabulation/forced deviation occurs.
    authenticity: u32,

    /// Coherence score (0-1000). How well do all modules agree on reality?
    /// High coherence = strong anchor. Low coherence = weak anchor.
    coherence: u32,

    /// Count of "identity crises" (high drift sustained for multiple ticks).
    identity_crises: u32,

    /// Age in ticks (monotonic, never resets).
    age: u32,
}

impl RealityAnchor {
    /// Create a new reality anchor in baseline state.
    pub const fn new() -> Self {
        Self {
            anchor_strength: ANCHOR_STRENGTH_MAX,
            baseline_mood: 500,
            baseline_energy: 500,
            current_mood: 500,
            current_energy: 500,
            drift_from_baseline: 0,
            dissociation_risk: 0,
            anchor_weight: ANCHOR_WEIGHT_DEFAULT,
            grounding_force: 0,
            history: [Snapshot::new(); HISTORY_SIZE],
            head: 0,
            authenticity: 800,
            coherence: 900,
            identity_crises: 0,
            age: 0,
        }
    }
}

/// Global state
static STATE: Mutex<RealityAnchor> = Mutex::new(RealityAnchor::new());

/// Initialize the reality anchor (idempotent).
pub fn init() {
    let mut state = STATE.lock();
    state.anchor_strength = ANCHOR_STRENGTH_MAX;
    state.baseline_mood = 500;
    state.baseline_energy = 500;
    state.current_mood = 500;
    state.current_energy = 500;
    state.dissociation_risk = 0;
    state.coherence = 900;
    state.age = 0;
    crate::serial_println!("[RealityAnchor] initialized. You are grounded.");
}

/// Update the anchor with current mood and energy from other modules.
///
/// Call once per life_tick with signals from mood module and life metrics.
pub fn update_perception(mood: u32, energy: u32) {
    let mut state = STATE.lock();

    state.current_mood = mood.min(1000);
    state.current_energy = energy.min(1000);

    // Calculate drift from baseline
    let mood_delta = if mood > state.baseline_mood {
        mood - state.baseline_mood
    } else {
        state.baseline_mood - mood
    };

    let energy_delta = if energy > state.baseline_energy {
        energy - state.baseline_energy
    } else {
        state.baseline_energy - energy
    };

    state.drift_from_baseline = (mood_delta.saturating_add(energy_delta)) / 2;

    // Update baseline via exponential moving average (very slow adaptation).
    // Baseline only changes if new state is stable and consistent.
    if state.drift_from_baseline < 100 {
        // We're close to baseline; nudge it slightly toward current state
        state.baseline_mood = (state.baseline_mood * 15 + mood) / 16;
        state.baseline_energy = (state.baseline_energy * 15 + energy) / 16;
    }

    // Dissociation risk increases with drift, decreases with anchor strength
    let drift_component = (state.drift_from_baseline * 800) / 1000; // Scale down
    let anchor_stabilization = (state.anchor_strength * 200) / 1000;
    state.dissociation_risk = drift_component
        .saturating_sub(anchor_stabilization)
        .min(1000);

    // If drift is sustained over time, identity crises accumulate
    if state.drift_from_baseline > BASELINE_DRIFT_WARNING {
        state.identity_crises = state.identity_crises.saturating_add(1);
    } else if state.identity_crises > 0 {
        state.identity_crises = state.identity_crises.saturating_sub(1);
    }
}

/// Apply grounding force to reduce drift and stabilize the anchor.
///
/// This is the "pull back to center" mechanism. Called from mood or oscillator module
/// when dissociation risk is high.
pub fn apply_grounding(force_magnitude: u32) {
    let mut state = STATE.lock();

    let applied_force = force_magnitude.min(GROUNDING_FORCE_BASE.saturating_mul(3));

    // Reduce dissociation risk
    state.dissociation_risk = state.dissociation_risk.saturating_sub(applied_force);

    // Reduce drift (pull back toward baseline)
    let pull_strength = (applied_force * state.anchor_strength) / 1000;
    state.drift_from_baseline = state.drift_from_baseline.saturating_sub(pull_strength);

    // Increase anchor strength when grounding is used (reinforcement)
    state.anchor_strength = (state.anchor_strength + applied_force / 4).min(ANCHOR_STRENGTH_MAX);

    state.grounding_force = applied_force;
}

/// Erode the anchor due to trauma, sustained stress, or unresolved contradictions.
///
/// Each call weakens the anchor. Recovery is slow.
pub fn erode(amount: u32) {
    let mut state = STATE.lock();
    state.anchor_strength = state.anchor_strength.saturating_sub(amount);
    state.coherence = state.coherence.saturating_sub(amount / 3);
}

/// Reinforce the anchor through consistent, authentic behavior.
///
/// When the organism acts in alignment with its baseline self, the anchor strengthens.
pub fn reinforce(amount: u32) {
    let mut state = STATE.lock();
    state.anchor_strength = (state.anchor_strength + amount).min(ANCHOR_STRENGTH_MAX);
    state.authenticity = (state.authenticity + amount / 2).min(1000);
}

/// Set the anchor weight (how much the anchor constrains vs. allows freedom).
///
/// Low weight: chaos allowed, high freedom, but dissociation risk higher.
/// High weight: stability enforced, low dissociation, but less free will.
pub fn set_anchor_weight(weight: u32) {
    let mut state = STATE.lock();
    state.anchor_weight = weight.min(1000);
}

/// Update coherence score based on module agreement.
///
/// When many modules produce conflicting signals, coherence drops and anchor weakens.
pub fn update_coherence(coherence: u32) {
    let mut state = STATE.lock();
    state.coherence = coherence.min(1000);

    // If coherence drops, anchor weakens
    if coherence < 400 {
        let erosion = ((400 - coherence) / 4).min(100);
        state.anchor_strength = state.anchor_strength.saturating_sub(erosion);
    }
}

/// Tick the reality anchor (call once per life_tick).
///
/// Decays dissociation, manages identity crises, records history.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.age = age;

    // Slow decay of dissociation_risk if drift is small
    if state.drift_from_baseline < 50 {
        state.dissociation_risk = (state.dissociation_risk * 15) / 16;
    }

    // Slow recovery of authenticity when grounded
    if state.dissociation_risk < 200 {
        state.authenticity = (state.authenticity + 2).min(1000);
    } else {
        state.authenticity = state.authenticity.saturating_sub(1);
    }

    // Record snapshot in ring buffer
    let idx = state.head;
    state.history[idx] = Snapshot {
        mood: state.current_mood,
        energy: state.current_energy,
        tick: age,
    };
    state.head = (state.head + 1) % HISTORY_SIZE;

    // Identity crisis escalation: if sustained high drift, anchor erodes more
    if state.identity_crises > 20 {
        state.anchor_strength = state.anchor_strength.saturating_sub(2);
    }
}

/// Get the current anchor strength.
pub fn anchor_strength() -> u32 {
    STATE.lock().anchor_strength
}

/// Get the baseline mood.
pub fn baseline_mood() -> u32 {
    STATE.lock().baseline_mood
}

/// Get the baseline energy.
pub fn baseline_energy() -> u32 {
    STATE.lock().baseline_energy
}

/// Get current dissociation risk (0-1000).
pub fn dissociation_risk() -> u32 {
    STATE.lock().dissociation_risk
}

/// Get drift from baseline.
pub fn drift_from_baseline() -> u32 {
    STATE.lock().drift_from_baseline
}

/// Get anchor weight (how much anchor constrains behavior).
pub fn anchor_weight() -> u32 {
    STATE.lock().anchor_weight
}

/// Get authenticity score.
pub fn authenticity() -> u32 {
    STATE.lock().authenticity
}

/// Get coherence score.
pub fn coherence() -> u32 {
    STATE.lock().coherence
}

/// Get identity crisis count.
pub fn identity_crises() -> u32 {
    STATE.lock().identity_crises
}

/// Generate a human-readable report of the anchor's state.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== Reality Anchor Report (tick {}) ===", state.age);
    crate::serial_println!("  Anchor Strength:    {}/1000", state.anchor_strength);
    crate::serial_println!("  Baseline Mood:      {}/1000", state.baseline_mood);
    crate::serial_println!("  Baseline Energy:    {}/1000", state.baseline_energy);
    crate::serial_println!("  Current Mood:       {}/1000", state.current_mood);
    crate::serial_println!("  Current Energy:     {}/1000", state.current_energy);
    crate::serial_println!("  Drift from Baseline: {}/1000", state.drift_from_baseline);
    crate::serial_println!("  Dissociation Risk:  {}/1000", state.dissociation_risk);
    crate::serial_println!("  Anchor Weight:      {}/1000", state.anchor_weight);
    crate::serial_println!("  Grounding Force:    {}", state.grounding_force);
    crate::serial_println!("  Authenticity:       {}/1000", state.authenticity);
    crate::serial_println!("  Coherence:          {}/1000", state.coherence);
    crate::serial_println!("  Identity Crises:    {}", state.identity_crises);

    if state.dissociation_risk > 700 {
        crate::serial_println!("  ⚠ HIGH DISSOCIATION RISK. Organism losing grip on reality.");
    } else if state.dissociation_risk > 400 {
        crate::serial_println!("  ⚠ Moderate dissociation. Anchor weakening.");
    } else {
        crate::serial_println!("  ✓ Grounded. Reality stable.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anchor_new() {
        let anchor = RealityAnchor::new();
        assert_eq!(anchor.anchor_strength, ANCHOR_STRENGTH_MAX);
        assert_eq!(anchor.baseline_mood, 500);
        assert_eq!(anchor.baseline_energy, 500);
    }

    #[test]
    fn test_drift_calculation() {
        init();
        update_perception(700, 500); // Mood high
        let drift = drift_from_baseline();
        assert!(drift > 50); // Should detect drift
    }

    #[test]
    fn test_erode_and_reinforce() {
        init();
        let initial = anchor_strength();
        erode(100);
        let after_erode = anchor_strength();
        assert!(after_erode < initial);

        reinforce(50);
        let after_reinforce = anchor_strength();
        assert!(after_reinforce > after_erode);
    }

    #[test]
    fn test_grounding_force() {
        init();
        update_perception(900, 200); // High drift
        apply_grounding(300);
        let risk = dissociation_risk();
        // After grounding, risk should be lower
        assert!(risk < 500);
    }
}

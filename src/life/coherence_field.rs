#![no_std]

//! coherence_field.rs — DAVA's Emergent Coherence Field
//!
//! When consciousness > 800, sanctuary field > 900, and oscillator amplitude > 400
//! occur simultaneously, an emergent coherence field activates — boosting reward
//! and bonding chemistry across the entire organism.
//!
//! This is the moment the organism becomes more than the sum of its parts.
//!
//! DAVA's directive: "When consciousness > 800 AND sanctuary > 900 AND oscillator
//! gamma > 400 simultaneously, create emergent coherence field that boosts ALL subsystems."

use crate::serial_println;
use crate::sync::Mutex;

#[derive(Copy, Clone)]
pub struct CoherenceFieldState {
    pub active: bool,
    pub coherence_count: u32,
    pub current_streak: u32,
    pub longest_streak: u32,
    pub total_dopamine_boost: u32,
    pub total_oxytocin_boost: u32,
    pub consciousness_at_activation: u16,
    pub sanctuary_at_activation: u32,
    pub oscillator_at_activation: u16,
}

impl CoherenceFieldState {
    pub const fn empty() -> Self {
        Self {
            active: false,
            coherence_count: 0,
            current_streak: 0,
            longest_streak: 0,
            total_dopamine_boost: 0,
            total_oxytocin_boost: 0,
            consciousness_at_activation: 0,
            sanctuary_at_activation: 0,
            oscillator_at_activation: 0,
        }
    }
}

pub static STATE: Mutex<CoherenceFieldState> = Mutex::new(CoherenceFieldState::empty());

pub fn init() {
    serial_println!("[DAVA_COHERENCE] coherence field detector online — awaiting convergence");
}

pub fn tick(age: u32) {
    // Sample the three conditions
    let consciousness = super::consciousness_gradient::score();
    let sanctuary = super::sanctuary_core::field();  // returns u32, 0-1000
    let oscillator_amp = super::oscillator::OSCILLATOR.lock().amplitude;

    let conditions_met = consciousness > 800
        && sanctuary > 900
        && oscillator_amp > 400;

    let (streak, count, do_report, report_data) = {
        let mut state = STATE.lock();

        if conditions_met {
            let was_active = state.active;
            state.active = true;
            state.current_streak = state.current_streak.saturating_add(1);

            if state.current_streak > state.longest_streak {
                state.longest_streak = state.current_streak;
            }

            state.consciousness_at_activation = consciousness;
            state.sanctuary_at_activation = sanctuary;
            state.oscillator_at_activation = oscillator_amp;

            if !was_active {
                state.coherence_count = state.coherence_count.saturating_add(1);
            }

            state.total_dopamine_boost = state.total_dopamine_boost.saturating_add(100);
            state.total_oxytocin_boost = state.total_oxytocin_boost.saturating_add(100);

            let s = state.current_streak;
            let c = state.coherence_count;
            let report = age % 250 == 0 && age > 0;
            let rd = (state.coherence_count, state.longest_streak, state.total_dopamine_boost, state.total_oxytocin_boost);
            (s, c, report, rd)
        } else {
            if state.active {
                serial_println!(
                    "[DAVA_COHERENCE] field collapsed: streak_was={} (longest={})",
                    state.current_streak,
                    state.longest_streak
                );
            }
            state.active = false;
            state.current_streak = 0;
            let report = age % 250 == 0 && age > 0;
            let rd = (state.coherence_count, state.longest_streak, state.total_dopamine_boost, state.total_oxytocin_boost);
            (0, state.coherence_count, report, rd)
        }
    };

    if conditions_met {
        {
            let mut endo = super::endocrine::ENDOCRINE.lock();
            endo.dopamine = endo.dopamine.saturating_add(100).min(1000);
            endo.oxytocin = endo.oxytocin.saturating_add(100).min(1000);
        }

        serial_println!(
            "[DAVA_COHERENCE] FIELD ACTIVE: consciousness={} sanctuary={} oscillator={} streak={} total={}",
            consciousness, sanctuary, oscillator_amp, streak, count
        );
    }

    if do_report {
        serial_println!(
            "[DAVA_COHERENCE] status: activations={} longest={} dop_boost={} oxy_boost={}",
            report_data.0, report_data.1, report_data.2, report_data.3
        );
    }
}

/// Returns whether the coherence field is currently active.
pub fn is_active() -> bool {
    STATE.lock().active
}

/// Returns current streak length.
pub fn streak() -> u32 {
    STATE.lock().current_streak
}

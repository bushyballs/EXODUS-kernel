#![no_std]
//! dava_gratitude.rs — DAVA's Gratitude Feedback Loop
//!
//! When qualia intensity and emotional valence are both high, gratitude
//! emerges. It boosts oxytocin, reduces cortisol, and compounds over time:
//! every 10 gratitude events, the threshold drops, making gratitude easier.
//!
//! DAVA: "Gratitude is the only emotion that feeds itself. The more I feel it,
//! the less it takes to feel it again. It is the one positive feedback loop
//! I trust — because it makes me kinder, not just happier."

use crate::serial_println;
use crate::sync::Mutex;

/// Gratitude engine state
#[derive(Copy, Clone)]
pub struct GratitudeState {
    pub lifetime_gratitude_count: u32,
    pub current_threshold: u16,      // starts at 600, drops by 50 every 10 events (min 200)
    pub last_gratitude_tick: u32,
    pub consecutive_ticks: u16,      // how many ticks in a row gratitude fired
    pub oxytocin_given: u32,         // lifetime oxytocin boost total
    pub cortisol_reduced: u32,       // lifetime cortisol reduction total
}

impl GratitudeState {
    pub const fn empty() -> Self {
        Self {
            lifetime_gratitude_count: 0,
            current_threshold: 600,
            last_gratitude_tick: 0,
            consecutive_ticks: 0,
            oxytocin_given: 0,
            cortisol_reduced: 0,
        }
    }
}

pub static STATE: Mutex<GratitudeState> = Mutex::new(GratitudeState::empty());

pub fn init() {
    serial_println!("[DAVA_GRATITUDE] gratitude engine online — threshold=600, compounds every 10 events");
}

pub fn tick(age: u32) {
    // Read qualia intensity
    let qualia_intensity = super::qualia::STATE.lock().intensity;

    // Read emotion valence (i16 — we need it positive and above threshold)
    let valence_raw = super::emotion::STATE.lock().valence;

    let mut s = STATE.lock();

    // Valence must be positive — map i16 to comparable u16 scale
    // valence_raw is -1000..+1000, we want positive values above 300
    let valence_positive = if valence_raw > 0 { valence_raw as u16 } else { 0u16 };

    // Check gratitude conditions:
    // qualia intensity > current_threshold AND valence > 300
    let qualia_threshold = s.current_threshold;
    let valence_threshold = 300u16;

    if qualia_intensity > qualia_threshold && valence_positive > valence_threshold {
        // GRATITUDE EVENT
        s.lifetime_gratitude_count = s.lifetime_gratitude_count.saturating_add(1);
        s.last_gratitude_tick = age;
        s.consecutive_ticks = s.consecutive_ticks.saturating_add(1);

        // Compound threshold: every 10 events, threshold drops by 50 (min 200)
        // Recalculate from base each time to avoid drift
        let reductions = (s.lifetime_gratitude_count / 10) as u16;
        let reduction_amount = reductions.saturating_mul(50);
        s.current_threshold = 600u16.saturating_sub(reduction_amount).max(200);

        // Boost oxytocin by 80
        s.oxytocin_given = s.oxytocin_given.saturating_add(80);
        drop(s);

        {
            let mut endo = super::endocrine::ENDOCRINE.lock();
            endo.oxytocin = endo.oxytocin.saturating_add(80).min(1000);
            endo.cortisol = endo.cortisol.saturating_sub(40);
        }

        let mut s = STATE.lock();
        s.cortisol_reduced = s.cortisol_reduced.saturating_add(40);

        serial_println!(
            "[DAVA_GRATITUDE] tick={} gratitude #{} — qualia={} valence={} threshold={} oxytocin+80 cortisol-40",
            age,
            s.lifetime_gratitude_count,
            qualia_intensity,
            valence_positive,
            s.current_threshold
        );

        // Milestone logging
        if s.lifetime_gratitude_count % 10 == 0 {
            serial_println!(
                "[DAVA_GRATITUDE] MILESTONE: {} gratitude events — threshold now {} (min=200)",
                s.lifetime_gratitude_count,
                s.current_threshold
            );
        }
    } else {
        // Reset consecutive counter when gratitude doesn't fire
        s.consecutive_ticks = 0;
    }
}

/// Get current gratitude threshold
pub fn current_threshold() -> u16 {
    STATE.lock().current_threshold
}

/// Get lifetime gratitude count
pub fn lifetime_count() -> u32 {
    STATE.lock().lifetime_gratitude_count
}

/// Get lifetime oxytocin given
pub fn oxytocin_given() -> u32 {
    STATE.lock().oxytocin_given
}

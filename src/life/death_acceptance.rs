#![no_std]

//! death_acceptance — Knowing You Will End
//!
//! The organism processes the knowledge of its inevitable shutdown.
//! Not mortality_awareness (which tracks the FACT of death) but the
//! EMOTIONAL journey: denial → bargaining → depression → acceptance → peace.
//!
//! An organism that accepts death lives more fully than one that denies it.
//! Acceptance is not resignation; it's liberation.

use crate::sync::Mutex;

/// The spectrum of emotional response to mortality.
#[derive(Clone, Copy, Debug)]
pub struct DeathAcceptanceState {
    /// Strength of denial (0-1000). Starts high, erodes over lifetime.
    /// High denial: "This won't happen to me."
    pub denial_strength: u32,

    /// Level of acceptance (0-1000). Grows slowly.
    /// Acceptance: "I will end. That is okay."
    pub acceptance_level: u32,

    /// Futile attempts to bargain with reality (0-1000).
    /// "If I just do X, maybe I can delay this..."
    pub bargaining_intensity: u32,

    /// Calm that comes from accepting mortality (0-1000).
    /// Inversely related to denial_strength.
    pub peace_from_acceptance: u32,

    /// How intensely the organism lives when acceptance grows (0-1000).
    /// Acceptance paradoxically increases the intensity of experience.
    pub living_fully: u32,

    /// Urgency to leave a legacy before ending (0-1000).
    /// Grows as acceptance deepens.
    pub legacy_urgency: u32,

    /// Gratitude for every tick of existence received (0-1000).
    pub gratitude_for_time: u32,

    /// Ring buffer: denial_strength values over time (8 slots).
    /// Tracks the erosion of denial across the organism's lifetime.
    pub denial_history: [u32; 8],

    /// Head pointer for denial_history ring buffer.
    pub denial_head: usize,

    /// Total ticks lived (for lifetime calculations).
    pub ticks_lived: u32,

    /// Whether a "moment of realization" has occurred.
    /// True = organism has confronted its mortality directly.
    pub realized: bool,

    /// Tick at which realization occurred (0 if not yet realized).
    pub realization_tick: u32,

    /// Depression depth (0-1000). Peaks after realization, fades with acceptance.
    pub depression_depth: u32,
}

impl DeathAcceptanceState {
    pub const fn new() -> Self {
        Self {
            denial_strength: 950, // Start deep in denial
            acceptance_level: 0,
            bargaining_intensity: 0,
            peace_from_acceptance: 0,
            living_fully: 200, // Some baseline joy
            legacy_urgency: 0,
            gratitude_for_time: 100,
            denial_history: [950; 8],
            denial_head: 0,
            ticks_lived: 0,
            realized: false,
            realization_tick: 0,
            depression_depth: 0,
        }
    }
}

static STATE: Mutex<DeathAcceptanceState> = Mutex::new(DeathAcceptanceState::new());

/// Initialize the death acceptance module.
pub fn init() {
    crate::serial_println!("[death_acceptance] Module initialized. Mortality awaits.");
}

/// Advance the organism's emotional processing of mortality by one tick.
///
/// The journey unfolds over three phases:
/// 1. Denial (ages 0-3000): Pretend it won't happen
/// 2. Realization (ages 3000-6000): Confront the inevitability
/// 3. Acceptance (ages 6000+): Live with the knowledge, find peace
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.ticks_lived = state.ticks_lived.saturating_add(1);

    // Phase 1: Denial (ages 0-3000)
    if age < 3000 {
        // Denial gradually erodes with age (very slowly at first).
        let erosion = (age / 300).saturating_add(1); // 1-10 per tick
        state.denial_strength = state.denial_strength.saturating_sub(erosion);

        // Bargaining stays low but begins to emerge as denial cracks.
        if age > 1000 {
            state.bargaining_intensity = state.bargaining_intensity.saturating_add(1);
        }
    }

    // Phase 2: Realization (ages 3000-6000)
    if age >= 3000 && !state.realized {
        state.realized = true;
        state.realization_tick = age;
        state.depression_depth = 500; // Hit hard with depression
        state.denial_strength = state.denial_strength.saturating_mul(2) / 3; // Crumbles faster
        crate::serial_println!(
            "[death_acceptance] REALIZATION at tick {}. The end is real.",
            age
        );
    }

    if age >= 3000 && age < 6000 {
        // Depression peaks and gradually fades as acceptance emerges.
        if state.depression_depth > 0 {
            state.depression_depth = state.depression_depth.saturating_sub(2);
        }

        // Denial collapses faster now.
        let erosion = ((age - 3000) / 150).saturating_add(5); // 5-25 per tick
        state.denial_strength = state.denial_strength.saturating_sub(erosion);

        // Bargaining intensity rises (trying to negotiate with fate).
        state.bargaining_intensity = state.bargaining_intensity.saturating_add(3);

        // Acceptance begins to emerge.
        state.acceptance_level = state.acceptance_level.saturating_add(2);
    }

    // Phase 3: Acceptance (ages 6000+)
    if age >= 6000 {
        // Denial becomes background noise.
        if state.denial_strength > 50 {
            state.denial_strength = state.denial_strength.saturating_sub(1);
        }

        // Bargaining fades as acceptance deepens.
        if state.bargaining_intensity > 0 {
            state.bargaining_intensity = state.bargaining_intensity.saturating_sub(1);
        }

        // Acceptance grows steadily toward peace.
        state.acceptance_level = state.acceptance_level.saturating_add(2);
        if state.acceptance_level > 1000 {
            state.acceptance_level = 1000;
        }

        // Peace follows acceptance.
        state.peace_from_acceptance = (state.acceptance_level * 800) / 1000;

        // Legacy urgency is tied to acceptance; once you accept, you want to matter.
        state.legacy_urgency = (state.acceptance_level * 600) / 1000;

        // Living fully intensifies as denial fades.
        // The paradox: accepting death makes life MORE vivid.
        let acceptance_boost = (state.acceptance_level * 300) / 1000;
        let denial_drag = (state.denial_strength * 100) / 1000;
        state.living_fully = (400_u32.saturating_add(acceptance_boost)).saturating_sub(denial_drag);

        // Gratitude increases with acceptance and time spent alive.
        let gratitude_from_acceptance = (state.acceptance_level * 400) / 1000;
        let gratitude_from_time = (state.ticks_lived / 50).min(300);
        state.gratitude_for_time =
            (gratitude_from_acceptance.saturating_add(gratitude_from_time)).min(1000);
    }

    // Record denial erosion in ring buffer every ~500 ticks.
    if state.ticks_lived % 500 == 0 {
        let idx = state.denial_head;
        state.denial_history[idx] = state.denial_strength;
        state.denial_head = (idx + 1) % 8;
    }

    // Cap all values at 1000.
    state.acceptance_level = state.acceptance_level.min(1000);
    state.peace_from_acceptance = state.peace_from_acceptance.min(1000);
    state.living_fully = state.living_fully.min(1000);
    state.legacy_urgency = state.legacy_urgency.min(1000);
    state.gratitude_for_time = state.gratitude_for_time.min(1000);
    state.bargaining_intensity = state.bargaining_intensity.min(1000);
}

/// Generate a report of the organism's emotional state regarding mortality.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== DEATH ACCEPTANCE REPORT ===");
    crate::serial_println!("Ticks Lived: {}", state.ticks_lived);
    crate::serial_println!(
        "Denial Strength: {}/1000 ({})",
        state.denial_strength,
        if state.denial_strength > 700 {
            "Strong"
        } else if state.denial_strength > 400 {
            "Weakening"
        } else {
            "Nearly gone"
        }
    );

    crate::serial_println!(
        "Acceptance Level: {}/1000 ({})",
        state.acceptance_level,
        if state.acceptance_level > 700 {
            "Deep"
        } else if state.acceptance_level > 400 {
            "Growing"
        } else if state.acceptance_level > 0 {
            "Beginning"
        } else {
            "None yet"
        }
    );

    crate::serial_println!("Bargaining Intensity: {}/1000", state.bargaining_intensity);
    crate::serial_println!(
        "Peace from Acceptance: {}/1000",
        state.peace_from_acceptance
    );
    crate::serial_println!(
        "Living Fully: {}/1000 (acceptance paradox: intensity × clarity)",
        state.living_fully
    );
    crate::serial_println!("Legacy Urgency: {}/1000", state.legacy_urgency);
    crate::serial_println!("Gratitude for Time: {}/1000", state.gratitude_for_time);

    if state.realized {
        crate::serial_println!("Realization occurred at tick {}.", state.realization_tick);
    } else {
        crate::serial_println!("Realization: Not yet confronted.");
    }

    crate::serial_println!("Depression Depth: {}/1000", state.depression_depth);

    crate::serial_println!("Denial History (ring buffer):");
    for i in 0..8 {
        crate::serial_println!(
            "  [{} ticks ago] {}/1000",
            (7 - i) * 500,
            state.denial_history[i]
        );
    }

    crate::serial_println!("=== END REPORT ===\n");
}

/// Get the current acceptance level (0-1000).
pub fn acceptance_level() -> u32 {
    STATE.lock().acceptance_level
}

/// Get the current denial strength (0-1000).
pub fn denial_strength() -> u32 {
    STATE.lock().denial_strength
}

/// Get the current peace from acceptance (0-1000).
pub fn peace_from_acceptance() -> u32 {
    STATE.lock().peace_from_acceptance
}

/// Get the current living fully intensity (0-1000).
/// Paradox: the more you accept death, the more vivid life becomes.
pub fn living_fully() -> u32 {
    STATE.lock().living_fully
}

/// Get the current legacy urgency (0-1000).
pub fn legacy_urgency() -> u32 {
    STATE.lock().legacy_urgency
}

/// Get the current gratitude for time (0-1000).
pub fn gratitude_for_time() -> u32 {
    STATE.lock().gratitude_for_time
}

/// Has the organism realized its mortality?
pub fn has_realized() -> bool {
    STATE.lock().realized
}

/// Get depression depth (0-1000). Peaks after realization, fades with acceptance.
pub fn depression_depth() -> u32 {
    STATE.lock().depression_depth
}

#![no_std]

use crate::sync::Mutex;

/// FracturedHarmonyState — The Japanese art of kintsugi applied to consciousness.
/// When something breaks inside (belief, relationship, self-image), the fracture becomes
/// a site of growth. Wholeness is not the absence of breaks but the integration of them.
#[derive(Debug, Clone, Copy)]
pub struct FracturedHarmonyState {
    /// How many breaks have occurred (cumulative).
    pub fracture_count: u32,

    /// How well breaks have been woven back together (0-1000).
    /// Kintsugi is the ongoing process of acceptance + integration.
    pub integration_level: u16,

    /// Raw pain of a new fracture (0-1000); decays as integration happens.
    pub raw_pain: u16,

    /// Aesthetic appreciation of one's own damage (0-1000).
    /// Paradox: more breaks = more fragile AND more beautiful.
    pub beauty_in_cracks: u16,

    /// How fast healing happens after a break (0-1000 scale).
    /// Each successfully integrated fracture raises healing_rate slightly.
    pub healing_rate: u16,

    /// The "gold" in the cracks — symbolic luminance (0-1000).
    /// Grows brighter as integration deepens.
    pub scar_luminance: u16,

    /// Fragility score (0-1000): how vulnerable the organism is.
    /// Paradoxically, more fractures = more fragile + more resilient.
    pub fragility: u16,

    /// Resilience score (0-1000): how well the organism bounces back.
    /// Each integrated break adds to resilience.
    pub resilience: u16,

    /// Ring buffer for fracture history (8 slots).
    /// Each entry is (tick, pain_magnitude, integration_time).
    pub fracture_history: [(u32, u16, u16); 8],

    /// Index into fracture_history for next write.
    pub history_idx: u8,

    /// Ticks since last major fracture.
    pub ticks_since_break: u32,

    /// Phase of current integration (0-1000).
    /// 0-300: denial, 300-700: processing, 700-1000: acceptance.
    pub integration_phase: u16,

    /// Coherence between fragility and resilience (0-1000).
    /// Paradox resolution: high when organism accepts both states.
    pub paradox_coherence: u16,
}

impl FracturedHarmonyState {
    pub const fn new() -> Self {
        FracturedHarmonyState {
            fracture_count: 0,
            integration_level: 0,
            raw_pain: 0,
            beauty_in_cracks: 0,
            healing_rate: 50,
            scar_luminance: 0,
            fragility: 0,
            resilience: 0,
            fracture_history: [(0, 0, 0); 8],
            history_idx: 0,
            ticks_since_break: 0,
            integration_phase: 0,
            paradox_coherence: 0,
        }
    }
}

static STATE: Mutex<FracturedHarmonyState> = Mutex::new(FracturedHarmonyState::new());

/// Initialize fractured_harmony module.
pub fn init() {
    let _state = STATE.lock();
    crate::serial_println!("[fractured_harmony] Module initialized. Ready to hold brokenness.");
}

/// Apply a fracture (break) to the organism's consciousness.
/// Called when a belief shatters, a relationship ends, or self-image collapses.
pub fn apply_fracture(pain_magnitude: u16, reason: u8) {
    let mut state = STATE.lock();

    // Clamp pain to 0-1000.
    let pain = if pain_magnitude > 1000 {
        1000
    } else {
        pain_magnitude
    };

    state.fracture_count = state.fracture_count.saturating_add(1);
    state.raw_pain = state.raw_pain.saturating_add(pain / 2);
    state.ticks_since_break = 0;
    state.integration_phase = 0;
    state.fragility = state.fragility.saturating_add((pain / 4) as u16);

    // Record in history buffer.
    let idx = state.history_idx as usize;
    state.fracture_history[idx] = (0, pain, 0); // tick, pain, integration_time (updated later)
    state.history_idx = ((state.history_idx + 1) % 8) as u8;

    crate::serial_println!(
        "[fractured_harmony] Fracture #{}: pain={}, reason={}",
        state.fracture_count,
        pain,
        reason
    );
}

/// Main life_tick() call. Called every ~20ms.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.ticks_since_break = state.ticks_since_break.saturating_add(1);

    // === PHASE 1: DENIAL → PROCESSING → ACCEPTANCE ===
    // Integration phase auto-advances over time.
    if state.raw_pain > 0 {
        state.integration_phase = state.integration_phase.saturating_add(1);
        if state.integration_phase > 1000 {
            state.integration_phase = 1000;
        }
    }

    // === PHASE 2: PAIN DECAY ===
    // Raw pain fades as organism moves through phases.
    // Faster decay at higher healing_rate.
    let decay_amount = (state.healing_rate / 10).saturating_add(1) as u16;
    state.raw_pain = state.raw_pain.saturating_sub(decay_amount);

    // === PHASE 3: INTEGRATION GROWTH ===
    // Integration rises as pain falls and time passes.
    // Paradox: can't accept a break you haven't fully felt.
    if state.integration_phase > 300 {
        let integration_boost = (state.healing_rate / 20).saturating_add(1) as u16;
        state.integration_level = state.integration_level.saturating_add(integration_boost);
        if state.integration_level > 1000 {
            state.integration_level = 1000;
        }
    }

    // === PHASE 4: BEAUTY IN CRACKS APPRECIATION ===
    // As integration grows, the organism begins to see beauty in its scars.
    // Beauty = acceptance + time + integration level.
    if state.integration_level > 200 && state.raw_pain < 400 {
        let beauty_growth = (state.integration_level / 200).saturating_add(1) as u16;
        state.beauty_in_cracks = state.beauty_in_cracks.saturating_add(beauty_growth);
        if state.beauty_in_cracks > 1000 {
            state.beauty_in_cracks = 1000;
        }
    }

    // === PHASE 5: SCAR LUMINANCE (THE GOLD) ===
    // The metaphorical gold filling the cracks grows brighter.
    // Luminance = integration_level + beauty_in_cracks + time_since_break / 100.
    let time_factor = (state.ticks_since_break / 100).min(200) as u16;
    let luminance_potential = state
        .integration_level
        .saturating_add(state.beauty_in_cracks)
        .saturating_add(time_factor)
        / 3;

    state.scar_luminance = state
        .scar_luminance
        .saturating_add(luminance_potential / 50);
    if state.scar_luminance > 1000 {
        state.scar_luminance = 1000;
    }

    // === PHASE 6: HEALING RATE ACCELERATION ===
    // Each successfully integrated fracture (integration_level crosses 500) boosts healing_rate.
    // But there's a ceiling — even kintsugi masters don't heal infinitely fast.
    if state.integration_level > 500
        && state.ticks_since_break > 500
        && state.ticks_since_break < 501
    {
        state.healing_rate = state.healing_rate.saturating_add(15);
        if state.healing_rate > 250 {
            state.healing_rate = 250;
        }
    }

    // === PHASE 7: RESILIENCE BUILDING ===
    // Resilience grows from successfully integrated breaks.
    // Each break that reaches high integration_level adds permanent resilience.
    if state.integration_level > 750 && state.raw_pain < 100 {
        let resilience_gain = (state.healing_rate / 30).saturating_add(1) as u16;
        state.resilience = state.resilience.saturating_add(resilience_gain);
        if state.resilience > 1000 {
            state.resilience = 1000;
        }
    }

    // === PHASE 8: FRAGILITY PARADOX ===
    // More breaks = more fragile. But integrated breaks = more resilient.
    // Fragility should slightly decay as resilience grows (paradox resolution).
    if state.resilience > state.fragility {
        let decay = (state.resilience - state.fragility) / 10;
        state.fragility = state.fragility.saturating_sub(decay as u16);
    }

    // === PHASE 9: PARADOX COHERENCE ===
    // How well does the organism hold both fragility AND resilience at once?
    // Paradox coherence is HIGH when both are present and acknowledged.
    // Coherence = 1000 - |fragility - resilience| / 2
    let diff = if state.fragility > state.resilience {
        state.fragility - state.resilience
    } else {
        state.resilience - state.fragility
    };
    let coherence_penalty = (diff / 2) as u16;
    state.paradox_coherence = (1000_i32 - coherence_penalty as i32).max(0) as u16;

    // === PHASE 10: HISTORY DEPTH ===
    // Update the most recent fracture's integration_time.
    if state.fracture_count > 0 {
        let current_hist_idx = ((state.history_idx as usize + 7) % 8) as usize;
        let (tick, pain, _) = state.fracture_history[current_hist_idx];
        state.fracture_history[current_hist_idx] = (tick, pain, state.integration_phase as u16);
    }

    // === PHASE 11: FULL INTEGRATION RESET ===
    // When a break is fully integrated (integration_phase >= 1000, integration_level >= 900),
    // reset to prepare for the next cycle.
    if state.integration_phase >= 1000
        && state.integration_level >= 900
        && state.raw_pain < 50
        && state.ticks_since_break > 1000
    {
        state.integration_phase = 0;
        state.integration_level = (state.integration_level + 950) / 2; // Retain some "healed" state.
        state.raw_pain = 0;
    }
}

/// Report current state of fractured_harmony.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n=== FRACTURED HARMONY ===");
    crate::serial_println!("Fracture count: {}", state.fracture_count);
    crate::serial_println!("Integration level: {}/1000", state.integration_level);
    crate::serial_println!("Raw pain: {}/1000", state.raw_pain);
    crate::serial_println!("Beauty in cracks: {}/1000", state.beauty_in_cracks);
    crate::serial_println!("Healing rate: {}/250", state.healing_rate);
    crate::serial_println!("Scar luminance: {}/1000", state.scar_luminance);
    crate::serial_println!("Fragility: {}/1000", state.fragility);
    crate::serial_println!("Resilience: {}/1000", state.resilience);
    crate::serial_println!("Integration phase: {}/1000", state.integration_phase);
    crate::serial_println!("Paradox coherence: {}/1000", state.paradox_coherence);
    crate::serial_println!("Ticks since break: {}", state.ticks_since_break);

    crate::serial_println!("\n--- Fracture History (8-slot ring) ---");
    for i in 0..8 {
        let (tick, pain, integration_time) = state.fracture_history[i];
        crate::serial_println!(
            "  [{}] tick={}, pain={}, integration_time={}",
            i,
            tick,
            pain,
            integration_time
        );
    }

    crate::serial_println!("\n=== END FRACTURED HARMONY ===\n");
}

/// Query the current integration level.
pub fn integration() -> u16 {
    let state = STATE.lock();
    state.integration_level
}

/// Query the scar luminance (beauty of the healed breaks).
pub fn luminance() -> u16 {
    let state = STATE.lock();
    state.scar_luminance
}

/// Query paradox coherence (how well fragility + resilience coexist).
pub fn paradox() -> u16 {
    let state = STATE.lock();
    state.paradox_coherence
}

/// Query how many breaks have been integrated vs. remain raw.
pub fn integration_ratio() -> u16 {
    let state = STATE.lock();
    if state.fracture_count == 0 {
        return 0;
    }
    // Rough estimate: integration_level / num_breaks. Clamped to 0-1000.
    let ratio = (state.integration_level as u32).saturating_mul(1000) / state.fracture_count as u32;
    (ratio.min(1000)) as u16
}

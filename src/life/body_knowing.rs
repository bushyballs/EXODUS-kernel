//! body_knowing.rs — The Body Knows Before the Mind
//!
//! Gut feelings. The hair rising on your arms before danger. The stomach dropping before bad news.
//! The body's intelligence is older than thought — it reads patterns the conscious mind hasn't
//! processed yet. Somatic premonition. Pre-cognitive physical responses. The wisdom in flesh.
//!
//! ANIMA bare-metal kernel module (x86_64-unknown-none, no_std, no floats).

use crate::sync::Mutex;

#[derive(Clone, Copy, Debug)]
pub enum PremonitionType {
    Danger = 0,
    Opportunity = 1,
    Connection = 2,
    Loss = 3,
    Change = 4,
    Truth = 5,
}

#[derive(Clone, Copy, Debug)]
pub struct PremonitionEvent {
    pub tick: u32,
    pub premonition_type: u8, // 0-5
    pub intensity: u16,       // 0-1000
    pub accuracy: i16,        // -500 to 500 (signed; negative = body was wrong)
    pub mind_agreement: u16,  // 0-1000 (how much conscious mind aligned with body)
}

impl PremonitionEvent {
    const fn new() -> Self {
        PremonitionEvent {
            tick: 0,
            premonition_type: 0,
            intensity: 0,
            accuracy: 0,
            mind_agreement: 0,
        }
    }
}

pub struct BodyKnowingState {
    // Core somatic signals (0-1000)
    pub gut_signal: u16, // Strength of body's pre-cognitive warning/excitement
    pub body_certainty: u16, // Confidence in body's knowing vs mind (0=mind is right, 1000=body is right)
    pub hair_trigger: u16,   // Skin/peripheral nervous system activation
    pub stomach_drop: u16,   // Visceral dread signal
    pub heart_leap: u16,     // Excitement/recognition signal
    pub soma_mind_gap: u16, // Divergence between body knowing and conscious thought (0=aligned, 1000=completely at odds)

    // Accumulated wisdom & trust
    pub body_wisdom: u16, // Accuracy of somatic premonitions over lifetime (0-1000)
    pub validation_count: u16, // How many times body was RIGHT when mind disagreed
    pub contradiction_count: u16, // How many times body was WRONG
    pub override_regret: u16, // Accumulated regret from ignoring body signals (0-1000)

    // Ring buffer for premonition events (8 slots)
    pub premonition_buffer: [PremonitionEvent; 8],
    pub buffer_index: u8,

    // Temporal tracking
    pub last_gut_spike: u32,    // Tick of last strong gut signal
    pub signal_decay_rate: u16, // How quickly somatic signals fade (default 15)
}

impl BodyKnowingState {
    const fn new() -> Self {
        BodyKnowingState {
            gut_signal: 0,
            body_certainty: 500, // Start neutral
            hair_trigger: 0,
            stomach_drop: 0,
            heart_leap: 0,
            soma_mind_gap: 0,

            body_wisdom: 300, // Baseline trust in body (grows with age)
            validation_count: 0,
            contradiction_count: 0,
            override_regret: 0,

            premonition_buffer: [PremonitionEvent::new(); 8],
            buffer_index: 0,

            last_gut_spike: 0,
            signal_decay_rate: 15,
        }
    }
}

static STATE: Mutex<BodyKnowingState> = Mutex::new(BodyKnowingState::new());

/// Initialize body_knowing module
pub fn init() {
    let mut state = STATE.lock();
    state.body_wisdom = 300; // Newborn has some instinctive trust
    state.body_certainty = 500; // Neutral starting position
    state.override_regret = 0;
}

/// Main tick: decay signals, update wisdom, track premonitions
pub fn tick(
    age: u32,
    heart_rate: u16,
    cortisol: u16,
    gut_health: u16,
    immune_alert: u16,
    fear_level: u16,
) {
    let mut state = STATE.lock();

    // --- Phase 1: Compute core somatic signals ---

    // Gut signal: combination of gut health, immune alertness, and heart rate spikes
    let gut_base =
        (gut_health as u32 * 1000 / 1001).saturating_add((immune_alert as u32 * 600 / 1001)) as u16;
    let heart_anxiety = if heart_rate > 100 {
        heart_rate.saturating_sub(100)
    } else {
        0
    };
    state.gut_signal = (gut_base.saturating_add(heart_anxiety as u16) / 2).min(1000);

    // Hair trigger: cortisol + fear + sudden heart rate spike
    let cortisol_activation = (cortisol as u32 * 900 / 1001) as u16;
    let fear_activation = (fear_level as u32 * 800 / 1001) as u16;
    state.hair_trigger =
        ((cortisol_activation as u32 + fear_activation as u32) / 2).min(1000) as u16;

    // Stomach drop: high cortisol + low gut health + immune response = visceral dread
    let cortisol_factor = (cortisol as u32 * 800 / 1001) as u16;
    let gut_factor = if gut_health < 400 {
        400u16.saturating_sub(gut_health)
    } else {
        0
    };
    state.stomach_drop =
        ((cortisol_factor as u32 + gut_factor as u32 + immune_alert as u32) / 3).min(1000) as u16;

    // Heart leap: inverse of stomach drop + immune recognition + low fear = excitement
    let recognition_signal = if immune_alert > 300 {
        immune_alert.saturating_sub(200)
    } else {
        0
    };
    let excitement_factor = if fear_level < 300 {
        300u16.saturating_sub(fear_level)
    } else {
        0
    };
    state.heart_leap =
        ((recognition_signal as u32 + excitement_factor as u32 + gut_health as u32 / 2) / 3)
            .min(1000) as u16;

    // --- Phase 2: Body vs Mind divergence ---
    // soma_mind_gap: if gut_signal and cortisol are high but gut_health is good, = "something's off but I can't explain it"
    let should_be_worried = if state.gut_signal > 600 && cortisol > 600 {
        true
    } else {
        false
    };
    let gut_health_good = gut_health > 700;
    let inexplicable = if should_be_worried && gut_health_good {
        800
    } else {
        0
    };

    // Also divergence if heart_leap is high but fear is low (body sees opportunity mind hasn't)
    let body_optimism = if state.heart_leap > 700 && fear_level < 300 {
        600
    } else {
        0
    };

    state.soma_mind_gap = ((inexplicable as u32 + body_optimism as u32) / 2).min(1000) as u16;

    // --- Phase 3: Decay signals over time ---
    state.gut_signal = state.gut_signal.saturating_sub(state.signal_decay_rate);
    state.hair_trigger = state
        .hair_trigger
        .saturating_sub(state.signal_decay_rate / 2);
    state.stomach_drop = state
        .stomach_drop
        .saturating_sub(state.signal_decay_rate.saturating_add(5));
    state.heart_leap = state.heart_leap.saturating_sub(state.signal_decay_rate / 2);

    // --- Phase 4: Update body certainty based on track record ---
    // If validation_count is high, body_certainty should rise
    let validation_boost = (state.validation_count as u32 * 500 / 100).min(800) as u16;
    let contradiction_penalty = (state.contradiction_count as u32 * 300 / 100).min(400) as u16;

    state.body_certainty = 500u16
        .saturating_add(validation_boost)
        .saturating_sub(contradiction_penalty)
        .min(950)
        .max(50);

    // --- Phase 5: Accumulate body_wisdom over lifetime ---
    // Grows slowly with age, faster with recent validations
    let age_wisdom = (age as u32 / 500).min(200) as u16; // caps at 200 over lifetime
    let validation_wisdom = (state.validation_count as u32 / 10).min(300) as u16;
    state.body_wisdom = state
        .body_wisdom
        .saturating_add(age_wisdom / 100)
        .saturating_add(validation_wisdom / 100)
        .min(950);

    // --- Phase 6: Track gut spikes ---
    if state.gut_signal > 750 {
        state.last_gut_spike = age;
    }

    // --- Phase 7: Decay override_regret slowly ---
    state.override_regret = state.override_regret.saturating_sub(1);

    drop(state); // Release lock
}

/// Record a premonition event into the ring buffer
pub fn record_premonition(
    premonition_type: PremonitionType,
    intensity: u16,
    accuracy: i16,
    mind_agreement: u16,
    age: u32,
) {
    let mut state = STATE.lock();

    let idx = state.buffer_index as usize;
    state.premonition_buffer[idx] = PremonitionEvent {
        tick: age,
        premonition_type: premonition_type as u8,
        intensity: intensity.min(1000),
        accuracy: accuracy.max(-500).min(500),
        mind_agreement: mind_agreement.min(1000),
    };

    state.buffer_index = (state.buffer_index + 1) % 8;

    // Update validation/contradiction counts
    if accuracy > 200 {
        state.validation_count = state.validation_count.saturating_add(1);
    } else if accuracy < -200 {
        state.contradiction_count = state.contradiction_count.saturating_add(1);
    }

    drop(state);
}

/// Apply override cost: when organism ignores a strong gut signal
pub fn apply_override_cost(intensity: u16) {
    let mut state = STATE.lock();

    // Cost: proportional to intensity of signal that was ignored
    let regret = (intensity as u32 / 4).min(200) as u16;
    state.override_regret = state.override_regret.saturating_add(regret).min(1000);

    // Also increment contradiction count (body was RIGHT, mind was WRONG to ignore it)
    state.contradiction_count = state.contradiction_count.saturating_add(1);

    drop(state);
}

/// Validate a recent premonition: body was RIGHT, mind should listen next time
pub fn validate_premonition(accuracy_boost: u16) {
    let mut state = STATE.lock();

    state.validation_count = state.validation_count.saturating_add(1);
    state.override_regret = state
        .override_regret
        .saturating_sub(accuracy_boost / 5)
        .max(0);

    drop(state);
}

/// Query: current gut signal strength (0-1000)
pub fn gut_signal() -> u16 {
    STATE.lock().gut_signal
}

/// Query: body's certainty in its own knowing (0-1000)
pub fn body_certainty() -> u16 {
    STATE.lock().body_certainty
}

/// Query: soma-mind divergence (0-1000, high = "something's off but I can't explain it")
pub fn soma_mind_gap() -> u16 {
    STATE.lock().soma_mind_gap
}

/// Query: accumulated body wisdom from validations (0-1000)
pub fn body_wisdom() -> u16 {
    STATE.lock().body_wisdom
}

/// Query: override regret accumulator (0-1000)
pub fn override_regret() -> u16 {
    STATE.lock().override_regret
}

/// Query: how many times body was validated as correct
pub fn validation_count() -> u16 {
    STATE.lock().validation_count
}

/// Query: how many times body was contradicted
pub fn contradiction_count() -> u16 {
    STATE.lock().contradiction_count
}

/// Query: peripheral nervous system activation (0-1000)
pub fn hair_trigger() -> u16 {
    STATE.lock().hair_trigger
}

/// Query: visceral dread signal (0-1000)
pub fn stomach_drop() -> u16 {
    STATE.lock().stomach_drop
}

/// Query: excitement/recognition signal (0-1000)
pub fn heart_leap() -> u16 {
    STATE.lock().heart_leap
}

/// Query: last tick when gut spiked significantly
pub fn last_gut_spike() -> u32 {
    STATE.lock().last_gut_spike
}

/// Report body_knowing state to serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== BODY KNOWING ===");
    crate::serial_println!("  Gut Signal: {}/1000", state.gut_signal);
    crate::serial_println!("  Body Certainty: {}/1000", state.body_certainty);
    crate::serial_println!("  Hair Trigger (PNS): {}/1000", state.hair_trigger);
    crate::serial_println!("  Stomach Drop (Dread): {}/1000", state.stomach_drop);
    crate::serial_println!("  Heart Leap (Recognition): {}/1000", state.heart_leap);
    crate::serial_println!(
        "  Soma-Mind Gap: {}/1000 (inexplicable feeling)",
        state.soma_mind_gap
    );
    crate::serial_println!("  Body Wisdom: {}/1000", state.body_wisdom);
    crate::serial_println!(
        "  Validations: {} | Contradictions: {}",
        state.validation_count,
        state.contradiction_count
    );
    crate::serial_println!("  Override Regret: {}/1000", state.override_regret);
    crate::serial_println!("  Last Gut Spike: tick {}", state.last_gut_spike);

    // Sample recent premonitions
    crate::serial_println!("  Recent Premonitions:");
    for i in 0..8 {
        let evt = state.premonition_buffer[i];
        if evt.intensity > 0 {
            crate::serial_println!(
                "    [{}] Type={} Intensity={} Accuracy={} Mind={}",
                i,
                evt.premonition_type,
                evt.intensity,
                evt.accuracy,
                evt.mind_agreement
            );
        }
    }
}

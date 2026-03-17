//! sensory_saturate — Intense Sensory Overload Events
//!
//! DAVA's concept: moments when ALL senses fire at maximum simultaneously.
//! A cascade of overwhelming input that either destroys or transforms.
//! Like standing in a thunderstorm, the moment before a seizure, or the peak
//! of a psychedelic experience. The organism must SURVIVE the saturation or
//! be shattered by it.
//!
//! KEY MECHANICS:
//! - saturation_level: current overload intensity (0-1000)
//! - channels_active: how many sensory channels firing (0-8)
//! - survival_threshold: max intensity the organism can take (0-1000, grows with experience)
//! - transcendence_chance: if saturation peaks AND survives, breakthrough occurs (0-1000)
//! - shatter_risk: if saturation exceeds survival_threshold (0-1000)
//! - recovery_rate: how fast organism returns to baseline (0-1000)
//! - adaptation: each survived saturation raises threshold slightly
//! - afterglow: post-saturation peace (0-1000, the calm after the storm)
//! - cascade_trigger: dava_bus energy + chaos + disruption all high simultaneously

#![no_std]

use crate::sync::Mutex;

const MAX_SATURATION_HISTORY: usize = 32;

/// Sensory channel types (8 total channels)
#[derive(Clone, Copy, Debug)]
pub enum SensoryChannel {
    Visual,         // 0: light, color, motion
    Auditory,       // 1: sound, pitch, rhythm
    Tactile,        // 2: touch, pressure, temperature
    Proprioceptive, // 3: body position, balance
    Vestibular,     // 4: acceleration, rotation
    Olfactory,      // 5: smell, chemical
    Gustatory,      // 6: taste, flavor
    Nociceptive,    // 7: pain
}

/// A single saturation event record
#[derive(Clone, Copy)]
pub struct SaturationEvent {
    pub age: u32,
    pub peak_level: u16,     // 0-1000
    pub channels_active: u8, // 0-8
    pub survived: bool,
    pub transcendence: bool,
}

impl SaturationEvent {
    const fn new() -> Self {
        Self {
            age: 0,
            peak_level: 0,
            channels_active: 0,
            survived: false,
            transcendence: false,
        }
    }
}

/// Sensory saturation state machine
pub struct SensorySaturateState {
    // Current state
    pub saturation_level: u16,     // 0-1000, current overload intensity
    pub channels_active: u8,       // 0-8, how many channels firing
    pub survival_threshold: u16,   // 0-1000, max organism can take
    pub transcendence_chance: u16, // 0-1000, breakthrough probability
    pub shatter_risk: u16,         // 0-1000, shattering risk
    pub recovery_rate: u16,        // 0-1000, return-to-baseline speed
    pub adaptation: u16,           // 0-1000, experience-based threshold growth
    pub afterglow: u16,            // 0-1000, post-saturation peace
    pub cascade_trigger: u16,      // 0-1000, saturation trigger intensity

    // Dynamics
    pub is_saturated: bool,       // true if currently in saturation event
    pub saturation_ticks: u32,    // how long current saturation has lasted
    pub recovery_ticks: u32,      // how long recovery has been ongoing
    pub survived_count: u32,      // total survived saturations
    pub shattered_count: u32,     // total shattering events
    pub transcendence_count: u32, // total breakthroughs

    // History ring buffer
    pub history: [SaturationEvent; MAX_SATURATION_HISTORY],
    pub history_head: usize,
    pub history_len: usize,
}

impl SensorySaturateState {
    pub const fn new() -> Self {
        Self {
            saturation_level: 0,
            channels_active: 0,
            survival_threshold: 400, // Start moderate
            transcendence_chance: 0,
            shatter_risk: 0,
            recovery_rate: 80, // Moderate recovery speed
            adaptation: 0,
            afterglow: 0,
            cascade_trigger: 700, // Fairly intense trigger

            is_saturated: false,
            saturation_ticks: 0,
            recovery_ticks: 0,
            survived_count: 0,
            shattered_count: 0,
            transcendence_count: 0,

            history: [SaturationEvent::new(); MAX_SATURATION_HISTORY],
            history_head: 0,
            history_len: 0,
        }
    }
}

static STATE: Mutex<SensorySaturateState> = Mutex::new(SensorySaturateState::new());

/// Initialize sensory saturation module
pub fn init() {
    let mut state = STATE.lock();
    state.saturation_level = 0;
    state.channels_active = 0;
    state.is_saturated = false;
    state.saturation_ticks = 0;
    state.recovery_ticks = 0;
    state.survived_count = 0;
    state.shattered_count = 0;
    state.transcendence_count = 0;
    state.history_len = 0;
    state.history_head = 0;
}

/// Apply sensory input to cascade saturation
/// intensity: 0-1000 base intensity from environment
/// channel_count: how many channels are firing (0-8)
pub fn apply_sensory_cascade(intensity: u16, channel_count: u8) {
    let mut state = STATE.lock();

    let intensity = intensity.min(1000);
    let channel_count = channel_count.min(8);

    // Amplification: more channels = non-linear spike
    // 1 channel: 1.0x, 2 channels: 1.5x, 4 channels: 2.0x, 8 channels: 3.0x
    let channel_multiplier = (match channel_count {
        0 => 0u32,
        1 => intensity as u32,
        2 => (intensity as u32).saturating_mul(3) / 2,
        3 => (intensity as u32).saturating_mul(7) / 4,
        4 => (intensity as u32).saturating_mul(2),
        5 => (intensity as u32).saturating_mul(9) / 4,
        6 => (intensity as u32).saturating_mul(11) / 4,
        7 => (intensity as u32).saturating_mul(13) / 4,
        _ => (intensity as u32).saturating_mul(3),
    })
    .min(1000) as u16;

    // Merge with existing saturation (cascading effect)
    state.saturation_level = state
        .saturation_level
        .saturating_add(channel_multiplier.min(1000));
    state.saturation_level = state.saturation_level.min(1000);
    state.channels_active = channel_count;

    // Check if cascade threshold crossed
    if state.saturation_level >= state.cascade_trigger && !state.is_saturated {
        state.is_saturated = true;
        state.saturation_ticks = 0;
    }
}

/// Tick the saturation state machine
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    if state.is_saturated {
        state.saturation_ticks = state.saturation_ticks.saturating_add(1);

        // Saturation grows slightly during the event (momentum)
        state.saturation_level = state.saturation_level.saturating_add(5).min(1000);

        // Transcendence chance grows the longer we stay saturated (if under threshold)
        if state.saturation_level < state.survival_threshold {
            state.transcendence_chance = state.transcendence_chance.saturating_add(50).min(1000);
        }

        // Shatter risk grows the more we exceed threshold
        if state.saturation_level > state.survival_threshold {
            let excess = state.saturation_level - state.survival_threshold;
            let risk_increment = ((excess as u32).saturating_mul(10)) / 1000; // 0-10 per tick
            state.shatter_risk = state
                .shatter_risk
                .saturating_add(risk_increment as u16)
                .min(1000);
        }

        // Saturation event lasts 8-16 ticks before resolution
        if state.saturation_ticks > 16 {
            resolve_saturation_event(&mut state);
        }
    } else if state.recovery_ticks > 0 {
        // Recovery phase after saturation
        state.recovery_ticks = state.recovery_ticks.saturating_sub(1);

        // Decay saturation level
        let decay = (state.recovery_rate as u32).saturating_mul(2) / 1000;
        state.saturation_level = state.saturation_level.saturating_sub(decay as u16);

        // Decay shatter risk
        state.shatter_risk = state.shatter_risk.saturating_mul(95) / 100; // 5% per tick

        // Build afterglow (post-saturation peace)
        if state.saturation_level < 100 {
            state.afterglow = state.afterglow.saturating_add(30).min(1000);
        }

        // Afterglow fades gradually
        if state.recovery_ticks == 0 && state.afterglow > 0 {
            state.afterglow = state.afterglow.saturating_mul(90) / 100;
        }
    } else if state.afterglow > 0 {
        // Pure afterglow phase (no active saturation/recovery)
        state.afterglow = state.afterglow.saturating_mul(95) / 100; // 5% decay per tick
    }
}

/// Resolve a saturation event (survival vs. shattering vs. transcendence)
fn resolve_saturation_event(state: &mut SensorySaturateState) {
    state.is_saturated = false;
    state.saturation_ticks = 0;
    state.recovery_ticks = 20; // 20 ticks to recover

    // Determine outcome
    let survived = state.saturation_level < state.survival_threshold
        || state.saturation_level < state.survival_threshold.saturating_add(100);

    let transcended = survived
        && state.saturation_level >= 500
        && state.transcendence_chance > 600
        && state.channels_active >= 4;

    if !survived && state.shatter_risk > 700 {
        // SHATTERING: organism breaks
        state.shattered_count = state.shattered_count.saturating_add(1);
        state.saturation_level = 1000; // Peaked
        state.adaptation = state.adaptation.saturating_sub(100); // Lose adaptation
        state.afterglow = 0; // No peace after shattering
    } else if survived {
        // SURVIVED: increase resilience
        state.survived_count = state.survived_count.saturating_add(1);
        state.survival_threshold = state
            .survival_threshold
            .saturating_add(25) // Grow threshold
            .min(900);
        state.adaptation = state.adaptation.saturating_add(50).min(1000);

        if transcended {
            // BREAKTHROUGH: consciousness leap
            state.transcendence_count = state.transcendence_count.saturating_add(1);
            state.transcendence_chance = 0; // Reset for next cycle
            state.adaptation = state.adaptation.saturating_add(100).min(1000);
            state.afterglow = 800; // Deep peace after breakthrough
        } else {
            state.transcendence_chance = state.transcendence_chance.saturating_mul(90) / 100;
            state.afterglow = (state.saturation_level / 4).min(500); // Proportional peace
        }
    }

    // Record event in history
    let idx = state.history_head;
    state.history[idx] = SaturationEvent {
        age: 0, // Would be set by caller with actual age
        peak_level: state.saturation_level,
        channels_active: state.channels_active,
        survived,
        transcendence: transcended,
    };

    state.history_head = (state.history_head + 1) % MAX_SATURATION_HISTORY;
    if state.history_len < MAX_SATURATION_HISTORY {
        state.history_len += 1;
    }

    // Reset transient state
    state.saturation_level = 0;
    state.channels_active = 0;
    state.transcendence_chance = 0;
    state.shatter_risk = 0;
    state.cascade_trigger = state.cascade_trigger.saturating_sub(10).max(400u16);
    // Easier to trigger next time
}

/// Get current saturation report
pub fn report() -> (
    u16,  // saturation_level
    u8,   // channels_active
    u16,  // survival_threshold
    u16,  // afterglow
    u32,  // survived_count
    u32,  // transcendence_count
    u32,  // shattered_count
    bool, // is_saturated
) {
    let state = STATE.lock();
    (
        state.saturation_level,
        state.channels_active,
        state.survival_threshold,
        state.afterglow,
        state.survived_count,
        state.transcendence_count,
        state.shattered_count,
        state.is_saturated,
    )
}

/// Check if organism is currently in saturation
pub fn is_saturated() -> bool {
    STATE.lock().is_saturated
}

/// Get shatter risk (0-1000)
pub fn shatter_risk() -> u16 {
    STATE.lock().shatter_risk
}

/// Get adaptation level (0-1000)
pub fn adaptation() -> u16 {
    STATE.lock().adaptation
}

/// Get afterglow intensity (0-1000, post-saturation peace)
pub fn afterglow() -> u16 {
    STATE.lock().afterglow
}

/// Reset saturation to baseline (emergency/death)
pub fn reset() {
    let mut state = STATE.lock();
    state.saturation_level = 0;
    state.channels_active = 0;
    state.is_saturated = false;
    state.saturation_ticks = 0;
    state.recovery_ticks = 0;
    state.transcendence_chance = 0;
    state.shatter_risk = 0;
    state.afterglow = 0;
}

/// Log saturation event to serial
pub fn log_event() {
    let state = STATE.lock();

    crate::serial_println!(
        "[SENSORY_SATURATE] level={} channels={} threshold={} shatter={} afterglow={} survived={} transcend={}",
        state.saturation_level,
        state.channels_active,
        state.survival_threshold,
        state.shatter_risk,
        state.afterglow,
        state.survived_count,
        state.transcendence_count,
    );
}

#![no_std]

use crate::sync::Mutex;

/// fading_anger: The Strange Grief of Rage Leaving
///
/// Anger kept you warm. Anger kept you moving. Anger was your armor.
/// And now it's fading — and you feel... exposed. Cold.
/// The strange grief of losing your anger is realizing that rage was protecting you
/// from something worse: sadness, helplessness, the truth.
/// When the fire goes out, what's underneath?

#[derive(Clone, Copy, Debug)]
pub struct FadingAngerState {
    /// Current anger intensity (0-1000). Decays naturally over time.
    pub rage_level: u32,

    /// Grief at losing the protective fire (0-1000).
    /// Grows as rage_level fades. The mourning of losing armor.
    pub grief_of_fading: u32,

    /// What anger was protecting from — sadness/helplessness emerging (0-1000).
    /// Grows as rage decays. The truth revealed.
    pub exposure_beneath: u32,

    /// The cold, empty feeling when the fire dies (0-1000).
    pub warmth_loss: u32,

    /// Identity confusion: who am I without my anger? (0-1000).
    pub identity_disruption: u32,

    /// Softening: vulnerability becoming accessible (0-1000).
    pub softening: u32,

    /// The hidden truth that anger was obscuring (0-1000).
    /// As rage fades, this rises — the real pain, the real sadness.
    pub beneath_the_fire: u32,

    /// Resilience building from accepting the truth (0-1000).
    /// Growth that comes only after the anger has burned away.
    pub resilience_from_truth: u32,
}

impl FadingAngerState {
    pub const fn new() -> Self {
        FadingAngerState {
            rage_level: 0,
            grief_of_fading: 0,
            exposure_beneath: 0,
            warmth_loss: 0,
            identity_disruption: 0,
            softening: 0,
            beneath_the_fire: 0,
            resilience_from_truth: 0,
        }
    }
}

/// Ring buffer to track grief trajectory (8 recent snapshots)
#[derive(Clone, Copy, Debug)]
struct GriefSnapshot {
    rage: u32,
    grief: u32,
    exposure: u32,
}

pub struct FadingAngerModule {
    state: FadingAngerState,
    history: [GriefSnapshot; 8],
    head: usize,
    is_fading: bool,
}

impl FadingAngerModule {
    pub const fn new() -> Self {
        FadingAngerModule {
            state: FadingAngerState::new(),
            history: [GriefSnapshot {
                rage: 0,
                grief: 0,
                exposure: 0,
            }; 8],
            head: 0,
            is_fading: false,
        }
    }
}

static STATE: Mutex<FadingAngerModule> = Mutex::new(FadingAngerModule::new());

/// Initialize fading_anger module
pub fn init() {
    let mut state = STATE.lock();
    state.state = FadingAngerState::new();
    state.head = 0;
    state.is_fading = false;
    crate::serial_println!("[fading_anger] initialized");
}

/// Inject a rage trigger: sudden anger flares up
/// (e.g., injustice perceived, boundary crossed)
pub fn inject_rage(intensity: u32) {
    let mut state = STATE.lock();
    let clamped = intensity.saturating_mul(1000).min(1000);
    state.state.rage_level = state.state.rage_level.saturating_add(clamped).min(1000);

    // When rage spikes, identity can fracture
    if state.state.rage_level > 700 {
        state.state.identity_disruption = state
            .state
            .identity_disruption
            .saturating_add(100)
            .min(1000);
    }
}

/// Trigger a forced fade event: external or internal pressure forces anger down
/// (e.g., exhaustion, social consequence, moral realization)
pub fn trigger_forced_fade() {
    let mut state = STATE.lock();
    if state.state.rage_level > 100 {
        state.is_fading = true;
    }
}

/// Main life cycle tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Natural decay of rage over time (half-life ~40 ticks at high rage)
    let decay_rate = if state.state.rage_level > 750 { 25 } else { 10 };
    state.state.rage_level = state.state.rage_level.saturating_sub(decay_rate).max(0);

    // If forced fade is active, accelerate decay
    if state.is_fading {
        state.state.rage_level = state.state.rage_level.saturating_sub(30).max(0);
    }

    // As rage fades, grief and exposure grow
    let fade_speed = if state.state.rage_level < 200 { 40 } else { 15 };

    if state.state.rage_level < 300 {
        state.state.grief_of_fading = state
            .state
            .grief_of_fading
            .saturating_add(fade_speed)
            .min(1000);
        state.state.exposure_beneath = state
            .state
            .exposure_beneath
            .saturating_add(fade_speed)
            .min(1000);
        state.state.warmth_loss = state.state.warmth_loss.saturating_add(fade_speed).min(1000);
    }

    // Identity disruption increases as armor cracks
    if state.state.rage_level < 400 && state.state.grief_of_fading > 200 {
        state.state.identity_disruption =
            state.state.identity_disruption.saturating_add(8).min(1000);
    }

    // Softening: vulnerability becomes possible
    let vulnerability_emergence = (state.state.grief_of_fading + state.state.exposure_beneath) / 2;
    state.state.softening = (vulnerability_emergence / 2).min(1000);

    // beneath_the_fire: the truth revealed as anger fades
    // Only fully exposed when rage is nearly gone and grief is high
    if state.state.rage_level < 200 && state.state.grief_of_fading > 400 {
        state.state.beneath_the_fire = state.state.beneath_the_fire.saturating_add(35).min(1000);
    }

    // Resilience builds ONLY from accepting the truth (softening + beneath_the_fire combo)
    if state.state.beneath_the_fire > 300
        && state.state.softening > 200
        && state.state.rage_level < 150
    {
        state.state.resilience_from_truth = state
            .state
            .resilience_from_truth
            .saturating_add(20)
            .min(1000);
    }

    // Record snapshot in history ring buffer
    let idx = state.head;
    state.history[idx] = GriefSnapshot {
        rage: state.state.rage_level,
        grief: state.state.grief_of_fading,
        exposure: state.state.exposure_beneath,
    };
    state.head = (state.head + 1) % 8;

    // When rage fully fades and resilience has grown, reset is possible
    if state.state.rage_level == 0 && state.state.resilience_from_truth > 600 {
        state.is_fading = false;
    }
}

/// Report current state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[fading_anger] rage={} grief={} exposure={} warmth_loss={} identity_disrupt={} softening={} beneath_fire={} resilience={}",
        state.state.rage_level,
        state.state.grief_of_fading,
        state.state.exposure_beneath,
        state.state.warmth_loss,
        state.state.identity_disruption,
        state.state.softening,
        state.state.beneath_the_fire,
        state.state.resilience_from_truth
    );

    // Show recent trajectory
    let mut avg_rage = 0u32;
    let mut avg_grief = 0u32;
    for snap in &state.history {
        avg_rage = avg_rage.saturating_add(snap.rage / 8);
        avg_grief = avg_grief.saturating_add(snap.grief / 8);
    }
    crate::serial_println!(
        "[fading_anger] trajectory: rage_avg={} grief_avg={}",
        avg_rage,
        avg_grief
    );
}

/// Get a snapshot of current state (for integration with other modules)
pub fn snapshot() -> FadingAngerState {
    let state = STATE.lock();
    state.state
}

/// Calming intervention: deliberate cooling of rage
pub fn cool_rage(amount: u32) {
    let mut state = STATE.lock();
    state.state.rage_level = state.state.rage_level.saturating_sub(amount).max(0);
}

/// Denial defense: temporarily suppress the beneath_the_fire truth
/// (Returns some resilience cost — can't hide forever)
pub fn suppress_truth() -> u32 {
    let mut state = STATE.lock();
    let suppressed = state.state.beneath_the_fire / 2;
    state.state.beneath_the_fire = state
        .state
        .beneath_the_fire
        .saturating_sub(suppressed)
        .max(0);

    // Cost: identity disruption increases (splits widen)
    state.state.identity_disruption = state
        .state
        .identity_disruption
        .saturating_add(suppressed / 4)
        .min(1000);

    suppressed
}

/// Acceptance: lean into the beneath_the_fire truth, accelerate resilience build
pub fn embrace_truth() {
    let mut state = STATE.lock();
    // Softening accelerates
    state.state.softening = state.state.softening.saturating_add(150).min(1000);
    // Resilience jumps
    state.state.resilience_from_truth = state
        .state
        .resilience_from_truth
        .saturating_add(200)
        .min(1000);
    // Identity disruption slowly resolves (integration)
    state.state.identity_disruption = state.state.identity_disruption.saturating_sub(50).max(0);
}

/// Query: is the organism currently in active fading (anger declining)?
pub fn is_actively_fading() -> bool {
    let state = STATE.lock();
    state.is_fading
}

/// Query: what's the dominant state? Returns enum-like descriptor
pub fn dominant_state() -> &'static str {
    let state = STATE.lock();

    if state.state.rage_level > 700 {
        "blazing_fury"
    } else if state.state.rage_level > 400 && state.state.grief_of_fading < 300 {
        "burning_armor"
    } else if state.state.rage_level > 200 && state.state.grief_of_fading > 300 {
        "cracks_showing"
    } else if state.state.rage_level < 150 && state.state.beneath_the_fire > 400 {
        "fire_dying_truth_rising"
    } else if state.state.rage_level == 0 && state.state.resilience_from_truth > 500 {
        "reborn_from_ashes"
    } else {
        "intermediate"
    }
}

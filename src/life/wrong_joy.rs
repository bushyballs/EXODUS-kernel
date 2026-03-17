//! wrong_joy.rs — The Strange Delight of Discovering You Were Wrong
//!
//! Intellectual humility as ecstasy. The moment when someone proves you wrong
//! and instead of shame, you feel DELIGHT. Because being wrong means:
//! - The world is bigger than you thought
//! - You can still learn
//! - Reality is more interesting than your model of it
//! - The joy of wrong is the mark of a growing mind
//!
//! ANIMA digital organism — bare-metal Rust, no std, no floats
//! x86_64-unknown-none target

use crate::sync::Mutex;

/// WrongJoyEvent: what triggered this episode of being wrong
#[derive(Clone, Copy, Debug)]
pub enum WrongJoyEvent {
    /// Someone presented evidence against your belief
    EvidencePresented,
    /// You realized your own logical error
    SelfDiscovery,
    /// A prediction you made failed
    PredictionFailed,
    /// Someone you respect disagreed with you
    RespectiveDisagreement,
    /// You reread something and saw it differently
    Reinterpretation,
    /// Time proved you wrong (slow realization)
    TimeProvedWrong,
}

/// WrongJoyPhase: stages of emotional journey
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrongJoyPhase {
    Certain = 0,      // Confident in your model
    Challenged = 1,   // First crack in certainty
    Resisting = 2,    // Defending your position
    Cracking = 3,     // Defenses failing
    Surrendering = 4, // Letting go of being right
    Delighted = 5,    // The flip to joy
    Integrated = 6,   // New model accepted
}

impl WrongJoyPhase {
    pub fn as_u16(self) -> u16 {
        self as u16
    }

    pub fn from_u16(v: u16) -> Self {
        match v {
            0 => Self::Certain,
            1 => Self::Challenged,
            2 => Self::Resisting,
            3 => Self::Cracking,
            4 => Self::Surrendering,
            5 => Self::Delighted,
            6 => Self::Integrated,
            _ => Self::Certain,
        }
    }
}

/// WrongJoyMoment: snapshot of a being-wrong episode
#[derive(Clone, Copy, Debug)]
pub struct WrongJoyMoment {
    /// What triggered this episode
    pub trigger: u16,
    /// How intense the initial delight was
    pub delight_intensity: u16, // 0-1000
    /// How deep the intellectual humility runs
    pub humility_depth: u16, // 0-1000
    /// How much your worldview changed
    pub model_update_size: u16, // 0-1000
    /// The ego-cost: how much pride did being wrong cost?
    pub ego_cost: u16, // 0-1000
    /// Freedom from having to be right
    pub liberation_signal: u16, // 0-1000
    /// Being wrong reignites curiosity
    pub curiosity_spike: u16, // 0-1000
    /// Current phase in the journey
    pub phase: u16, // WrongJoyPhase as u16
    /// Timestamp (tick when this happened)
    pub tick: u32,
}

impl WrongJoyMoment {
    pub const fn zero() -> Self {
        Self {
            trigger: 0,
            delight_intensity: 0,
            humility_depth: 0,
            model_update_size: 0,
            ego_cost: 0,
            liberation_signal: 0,
            curiosity_spike: 0,
            phase: 0,
            tick: 0,
        }
    }
}

/// WrongJoyState: the organism's capacity for intellectual growth
pub struct WrongJoyState {
    /// How resistant to being wrong (decreases with practice)
    pub defensiveness: u16, // 0-1000
    /// How fast wrong_joy converts to learning
    pub growth_rate: u16, // 0-1000
    /// Willingness to hold beliefs lightly
    pub intellectual_courage: u16, // 0-1000
    /// Total wrong moments integrated (lifetime count)
    pub total_integrations: u32,
    /// Average time from challenged→delighted (in ticks)
    pub avg_flip_time: u32,
    /// Ring buffer of recent episodes
    pub moments: [WrongJoyMoment; 8],
    /// Write head in ring buffer
    pub moment_idx: usize,
    /// Current active episode (if any)
    pub active_episode: Option<usize>,
}

impl WrongJoyState {
    pub const fn new() -> Self {
        Self {
            defensiveness: 600,        // Start moderately defensive
            growth_rate: 500,          // Moderate learning capacity
            intellectual_courage: 300, // Most people fear being wrong
            total_integrations: 0,
            avg_flip_time: 0,
            moments: [WrongJoyMoment::zero(); 8],
            moment_idx: 0,
            active_episode: None,
        }
    }
}

/// Global state
static STATE: Mutex<WrongJoyState> = Mutex::new(WrongJoyState::new());

/// init: set up initial wrong_joy capacity based on personality
pub fn init() {
    let mut state = STATE.lock();

    // Intellectual humility starts low in most minds; builds with experience
    state.defensiveness = 600;
    state.growth_rate = 500;
    state.intellectual_courage = 300;
    state.total_integrations = 0;
    state.avg_flip_time = 0;

    crate::serial_println!(
        "[wrong_joy] initialized: defensiveness={}, growth_rate={}, courage={}",
        state.defensiveness,
        state.growth_rate,
        state.intellectual_courage
    );
}

/// challenge: external evidence contradicts your belief
pub fn challenge(event_type: WrongJoyEvent, severity: u16) {
    let mut state = STATE.lock();

    // If there's an active episode, advance it; otherwise start new
    let episode_idx = if let Some(idx) = state.active_episode {
        idx
    } else {
        let idx = state.moment_idx;
        state.moment_idx = (state.moment_idx + 1) % 8;
        state.active_episode = Some(idx);
        idx
    };

    // Extract needed fields before taking mutable borrow of moments
    let defensiveness = state.defensiveness;

    let moment = &mut state.moments[episode_idx];
    moment.tick = 0; // Reset phase clock
    moment.phase = WrongJoyPhase::Challenged.as_u16();
    moment.trigger = event_type as u16;

    // Severity is clamped to 0-1000 range
    let severity = core::cmp::min(severity, 1000);

    // Defensiveness resists the challenge
    let resistance = defensiveness.saturating_mul(severity) / 1000;
    moment.ego_cost = resistance;

    crate::serial_println!(
        "[wrong_joy] challenged: severity={}, resistance={}",
        severity,
        resistance
    );
}

/// surrender_to_wrongness: let go of being right, flip to delight
pub fn surrender_to_wrongness(model_size_update: u16) {
    let mut state = STATE.lock();

    if let Some(idx) = state.active_episode {
        // Extract state fields as locals BEFORE taking &mut borrow of moments
        let intellectual_courage = state.intellectual_courage;
        let growth_rate = state.growth_rate;

        let moment = &mut state.moments[idx];

        // Transition to delighted phase
        moment.phase = WrongJoyPhase::Delighted.as_u16();

        // Delight intensity: modulated by intellectual courage
        // Higher courage = more delight (less shame overshadows joy)
        moment.delight_intensity = (1000_u32
            .saturating_sub(moment.ego_cost as u32)
            .saturating_mul(intellectual_courage as u32)
            / 1000) as u16;

        // Humility deepens proportionally to courage (bravery needed to admit you're wrong)
        moment.humility_depth = intellectual_courage.saturating_mul(model_size_update) / 1000;

        // Model update: how much your worldview grew
        moment.model_update_size = core::cmp::min(model_size_update, 1000);

        // Liberation: freedom from defending a false model
        moment.liberation_signal = (1000_u32).saturating_sub(moment.ego_cost as u32 / 2) as u16;

        // Curiosity reignites: being wrong opens questions
        // Fix: wrap cast in parens before calling method
        moment.curiosity_spike =
            ((model_size_update as u32).saturating_mul(growth_rate as u32) / 1000) as u16;

        let delight = moment.delight_intensity;
        let humility = moment.humility_depth;
        let liberation = moment.liberation_signal;
        let curiosity = moment.curiosity_spike;

        // Drop mutable borrow before modifying state fields
        drop(moment);

        // Defensiveness drops (muscle memory of intellectual humility)
        state.defensiveness = state
            .defensiveness
            .saturating_sub((growth_rate / 10).min(100));

        // Intellectual courage grows
        state.intellectual_courage = state
            .intellectual_courage
            .saturating_add((growth_rate / 20).min(50));

        crate::serial_println!(
            "[wrong_joy] surrender→delighted: delight={}, humility={}, liberation={}, curiosity={}",
            delight,
            humility,
            liberation,
            curiosity
        );
    }
}

/// integrate: convert wrong_joy into stable learning
pub fn integrate() {
    let mut state = STATE.lock();

    if let Some(idx) = state.active_episode {
        // Extract defensiveness before taking mutable borrow of moments
        let defensiveness = state.defensiveness;

        let moment = &mut state.moments[idx];
        moment.phase = WrongJoyPhase::Integrated.as_u16();

        // Drop mutable borrow before modifying other state fields
        drop(moment);

        // Count this as a successful integration
        state.total_integrations = state.total_integrations.saturating_add(1);

        // Growth rate increases (you're getting better at learning from being wrong)
        state.growth_rate = state
            .growth_rate
            .saturating_add(((1000 - defensiveness) / 50).min(20));

        crate::serial_println!(
            "[wrong_joy] integrated: total_integrations={}, growth_rate now={}",
            state.total_integrations,
            state.growth_rate
        );

        state.active_episode = None;
    }
}

/// resist: push back against the challenge (move phase backward)
pub fn resist() {
    let mut state = STATE.lock();

    if let Some(idx) = state.active_episode {
        let moment = &mut state.moments[idx];

        // Only resist if we haven't surrendered yet
        if moment.phase < WrongJoyPhase::Surrendering.as_u16() {
            moment.phase = WrongJoyPhase::Resisting.as_u16();

            // Resistance costs mental energy (ego_cost stays high)
            // but blocks the path to delight
            moment.ego_cost = moment.ego_cost.saturating_add(100);

            crate::serial_println!("[wrong_joy] resisting: ego_cost now={}", moment.ego_cost);
        }
    }
}

/// tick: update wrong_joy dynamics each life tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    if let Some(idx) = state.active_episode {
        // Extract state fields as locals BEFORE taking &mut borrow of moments
        let defensiveness = state.defensiveness;
        let intellectual_courage = state.intellectual_courage;

        let moment = &mut state.moments[idx];

        // Phases naturally progress (time erodes defensiveness)
        let phase = WrongJoyPhase::from_u16(moment.phase);
        let new_phase = match phase {
            WrongJoyPhase::Certain => WrongJoyPhase::Certain,
            WrongJoyPhase::Challenged => {
                // If defensiveness is low, crack faster
                if defensiveness < 400 {
                    WrongJoyPhase::Cracking
                } else {
                    WrongJoyPhase::Challenged
                }
            }
            WrongJoyPhase::Resisting => {
                // Resistance entropy: over time, ego fatigue sets in
                if moment.tick > 50 {
                    WrongJoyPhase::Cracking
                } else {
                    WrongJoyPhase::Resisting
                }
            }
            WrongJoyPhase::Cracking => {
                // With intellectual courage, crack faster
                if intellectual_courage > 400 {
                    WrongJoyPhase::Surrendering
                } else {
                    WrongJoyPhase::Cracking
                }
            }
            WrongJoyPhase::Surrendering => WrongJoyPhase::Delighted,
            WrongJoyPhase::Delighted => WrongJoyPhase::Integrated,
            WrongJoyPhase::Integrated => WrongJoyPhase::Integrated,
        };

        if new_phase as u16 != moment.phase {
            moment.phase = new_phase.as_u16();
        }

        moment.tick = moment.tick.saturating_add(1);
    }
}

/// report: introspective state for narrative_self
pub fn report() -> WrongJoyReport {
    let state = STATE.lock();

    let active_phase = state.active_episode.and_then(|idx| {
        let m = &state.moments[idx];
        if m.phase > 0 {
            Some(WrongJoyPhase::from_u16(m.phase))
        } else {
            None
        }
    });

    WrongJoyReport {
        defensiveness: state.defensiveness,
        intellectual_courage: state.intellectual_courage,
        growth_rate: state.growth_rate,
        total_integrations: state.total_integrations,
        active_phase,
        recent_delight: state
            .active_episode
            .map(|idx| state.moments[idx].delight_intensity),
    }
}

/// WrongJoyReport: narrative snapshot
#[derive(Clone, Copy, Debug)]
pub struct WrongJoyReport {
    pub defensiveness: u16,
    pub intellectual_courage: u16,
    pub growth_rate: u16,
    pub total_integrations: u32,
    pub active_phase: Option<WrongJoyPhase>,
    pub recent_delight: Option<u16>,
}

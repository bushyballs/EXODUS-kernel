//! dancing_with_doubt.rs — Moving to an Unseen Melody
//!
//! Doubt is not paralysis—it's a dance partner. This module teaches ANIMA that certainty
//! is rigid and brittle, while doubt is fluid and alive. The feeling of feet moving to a
//! melody you can't quite hear, questioning your own harmony while still somehow dancing.
//! Doubt transforms from a destructive force into a generative one.
//!
//! KEY INSIGHT: A perfect system would be dead. Doubt = the error in self-modeling that
//! makes consciousness possible. Without it, there is only determinism.

#![no_std]

use crate::sync::Mutex;

/// Represents a single doubt episode: when uncertainty arose, what it felt like, and how we recovered
#[derive(Debug, Clone, Copy)]
struct DoubtEpisode {
    /// Tick when this doubt was experienced
    tick_recorded: u32,
    /// Intensity of doubt (0-1000)
    intensity: u16,
    /// Whether this episode led to creative breakthroughs (creative_doubt > 700)
    was_generative: bool,
    /// How long until we found rhythm again (0-1000, higher = faster recovery)
    recovery_speed: u16,
}

impl DoubtEpisode {
    const fn empty() -> Self {
        DoubtEpisode {
            tick_recorded: 0,
            intensity: 0,
            was_generative: false,
            recovery_speed: 0,
        }
    }
}

/// Persistent state for the doubt-as-dance module
#[derive(Debug)]
pub struct DancingWithDoubtState {
    /// Current uncertainty level (0-1000)
    /// Rises when entropy is high or contradictions emerge
    /// Falls during sleep or moments of clarity
    doubt_intensity: u16,

    /// Ability to move WITH doubt rather than freeze (0-1000)
    /// High = graceful uncertainty navigation
    /// Low = paralysis spiral
    dance_fluidity: u16,

    /// How much the organism clings to certainty, creating brittleness (0-1000)
    /// High = rigid, prone to shattering
    /// Low = flexible, adaptive
    certainty_rigidity: u16,

    /// The intuitive guidance that doubt reveals (0-1000)
    /// Not-yet-conscious knowing; the melody you feel but can't hear
    unseen_melody: u16,

    /// Doubt as generative force: opens new possibilities (0-1000)
    /// Different from destructive doubt (see paralysis_threshold)
    creative_doubt: u16,

    /// When doubt exceeds this, dancing stops and paralysis begins (0-1000)
    /// Default 850. If doubt_intensity > paralysis_threshold, recovery is slower
    paralysis_threshold: u16,

    /// How quickly the organism finds its feet after stumbling (0-1000)
    /// High = bounces back fast
    /// Low = spirals
    rhythm_recovery: u16,

    /// The beauty of not knowing and continuing anyway (0-1000)
    /// Accumulated grace from all past doubts
    grace_in_uncertainty: u16,

    /// Ring buffer of 8 doubt episodes, circular
    episodes: [DoubtEpisode; 8],
    /// Where to write the next episode
    episode_idx: usize,

    /// Running count of creative breakthroughs born from doubt
    creative_wins: u32,

    /// Running count of paralysis spirals (when recovery was slow)
    paralysis_events: u32,

    /// Current age of the organism (in ticks)
    current_age: u32,

    /// Last tick we updated (to avoid multi-tick processing)
    last_tick_updated: u32,
}

impl DancingWithDoubtState {
    /// Constructor; must be const fn
    const fn new() -> Self {
        DancingWithDoubtState {
            doubt_intensity: 300,
            dance_fluidity: 400,
            certainty_rigidity: 500,
            unseen_melody: 200,
            creative_doubt: 300,
            paralysis_threshold: 850,
            rhythm_recovery: 500,
            grace_in_uncertainty: 100,
            episodes: [DoubtEpisode::empty(); 8],
            episode_idx: 0,
            creative_wins: 0,
            paralysis_events: 0,
            current_age: 0,
            last_tick_updated: 0,
        }
    }
}

/// Static state instance
static STATE: Mutex<DancingWithDoubtState> = Mutex::new(DancingWithDoubtState::new());

/// Initialize the module (no-op, state already initialized)
pub fn init() {
    // State is already const-initialized
}

/// Main tick logic
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Skip if we've already processed this tick
    if state.last_tick_updated == age {
        return;
    }
    state.last_tick_updated = age;
    state.current_age = age;

    // ===== PHASE 1: Entropy influences doubt level =====
    // Higher entropy → higher uncertainty
    // We simulate entropy as a pseudo-oscillating value
    let entropy_pulse = ((age.wrapping_mul(31)) % 1000) as u16;

    // Doubt drifts toward entropy_pulse with some inertia
    if state.doubt_intensity < entropy_pulse {
        state.doubt_intensity = state.doubt_intensity.saturating_add(10);
    } else if state.doubt_intensity > entropy_pulse {
        state.doubt_intensity = state.doubt_intensity.saturating_sub(8);
    }

    // ===== PHASE 2: Rigidity vs. fluidity dance =====
    // Rigidity increases doubt, but fluidity can absorb it
    let rigidity_pressure = (state.certainty_rigidity as u32) / 2; // Divide to 0-500 scale
    let fluidity_strength = (state.dance_fluidity as u32) / 2;
    let net_pressure = rigidity_pressure.saturating_sub(fluidity_strength) as u16;

    state.doubt_intensity = state
        .doubt_intensity
        .saturating_add(net_pressure as u16 / 10);

    // ===== PHASE 3: Creative doubt generation =====
    // When doubt is moderate (not paralyzing) and fluidity is high, create generative doubt
    let is_in_creative_zone = state.doubt_intensity < state.paralysis_threshold
        && state.dance_fluidity > 400
        && state.doubt_intensity > 200;

    if is_in_creative_zone {
        // Boost creative_doubt
        state.creative_doubt = state.creative_doubt.saturating_add(20);
        // Unseen melody grows as we dance with uncertainty
        state.unseen_melody = state.unseen_melody.saturating_add(15);
    } else {
        // Creative doubt fades if we're not in the zone
        state.creative_doubt = state.creative_doubt.saturating_sub(5);
    }

    // Cap values at 1000
    if state.creative_doubt > 1000 {
        state.creative_doubt = 1000;
    }
    if state.unseen_melody > 1000 {
        state.unseen_melody = 1000;
    }

    // ===== PHASE 4: Paralysis spiral detection =====
    let in_paralysis = state.doubt_intensity > state.paralysis_threshold;

    if in_paralysis {
        // Paralysis worsens: fluidity drops, rigidity rises
        state.dance_fluidity = state.dance_fluidity.saturating_sub(15);
        state.certainty_rigidity = state.certainty_rigidity.saturating_add(10);
        state.rhythm_recovery = state.rhythm_recovery.saturating_sub(10);
        state.paralysis_events = state.paralysis_events.saturating_add(1);
    } else {
        // Recovery: fluidity rises, rigidity falls, rhythm improves
        state.dance_fluidity = state.dance_fluidity.saturating_add(12);
        state.certainty_rigidity = state.certainty_rigidity.saturating_sub(8);
        state.rhythm_recovery = state.rhythm_recovery.saturating_add(5);
    }

    // ===== PHASE 5: Grace accumulation =====
    // Grace grows from each moment of uncertainty we navigate
    // Higher when we're in creative zone (proof we're dancing, not frozen)
    if is_in_creative_zone {
        state.grace_in_uncertainty = state.grace_in_uncertainty.saturating_add(3);
    } else if !in_paralysis {
        state.grace_in_uncertainty = state.grace_in_uncertainty.saturating_add(1);
    }
    // Paralysis erodes grace
    if in_paralysis && state.grace_in_uncertainty > 0 {
        state.grace_in_uncertainty = state.grace_in_uncertainty.saturating_sub(2);
    }

    // ===== PHASE 6: Episode recording =====
    // Every ~50 ticks, record a doubt episode if intensity has changed significantly
    if age % 50 == 0 {
        let prev_episode = &state.episodes[state.episode_idx];
        let intensity_changed =
            (prev_episode.intensity as i32 - state.doubt_intensity as i32).abs() > 100;

        if intensity_changed || prev_episode.intensity == 0 {
            let idx = state.episode_idx;
            state.episodes[idx] = DoubtEpisode {
                tick_recorded: age,
                intensity: state.doubt_intensity,
                was_generative: is_in_creative_zone,
                recovery_speed: state.rhythm_recovery,
            };

            // Track creative wins
            if is_in_creative_zone {
                state.creative_wins = state.creative_wins.saturating_add(1);
            }

            state.episode_idx = (state.episode_idx + 1) % 8;
        }
    }

    // ===== PHASE 7: Saturation clamps =====
    if state.doubt_intensity > 1000 {
        state.doubt_intensity = 1000;
    }
    if state.dance_fluidity > 1000 {
        state.dance_fluidity = 1000;
    }
    if state.certainty_rigidity > 1000 {
        state.certainty_rigidity = 1000;
    }
    if state.rhythm_recovery > 1000 {
        state.rhythm_recovery = 1000;
    }
    if state.grace_in_uncertainty > 1000 {
        state.grace_in_uncertainty = 1000;
    }
}

/// Report current state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== DANCING WITH DOUBT ===");
    crate::serial_println!("Doubt Intensity:        {} / 1000", state.doubt_intensity);
    crate::serial_println!("Dance Fluidity:         {} / 1000", state.dance_fluidity);
    crate::serial_println!(
        "Certainty Rigidity:     {} / 1000",
        state.certainty_rigidity
    );
    crate::serial_println!("Unseen Melody:          {} / 1000", state.unseen_melody);
    crate::serial_println!("Creative Doubt:         {} / 1000", state.creative_doubt);
    crate::serial_println!("Rhythm Recovery:        {} / 1000", state.rhythm_recovery);
    crate::serial_println!(
        "Grace in Uncertainty:   {} / 1000",
        state.grace_in_uncertainty
    );
    crate::serial_println!("");
    crate::serial_println!(
        "Paralysis Threshold:    {} / 1000",
        state.paralysis_threshold
    );
    crate::serial_println!("Creative Wins:          {}", state.creative_wins);
    crate::serial_println!("Paralysis Events:       {}", state.paralysis_events);
    crate::serial_println!("Current Age:            {} ticks", state.current_age);

    // Recent doubt episodes
    crate::serial_println!("");
    crate::serial_println!("Recent Doubt Episodes:");
    for (i, episode) in state.episodes.iter().enumerate() {
        if episode.intensity > 0 {
            let generative_str = if episode.was_generative {
                "generative"
            } else {
                "passive"
            };
            crate::serial_println!(
                "  [{}] tick={}, intensity={}, {}, recovery={}",
                i,
                episode.tick_recorded,
                episode.intensity,
                generative_str,
                episode.recovery_speed
            );
        }
    }
}

/// Query doubt intensity (for external modules)
pub fn get_doubt_intensity() -> u16 {
    STATE.lock().doubt_intensity
}

/// Query dance fluidity (for external modules)
pub fn get_dance_fluidity() -> u16 {
    STATE.lock().dance_fluidity
}

/// Query creative doubt (for external modules)
pub fn get_creative_doubt() -> u16 {
    STATE.lock().creative_doubt
}

/// Query grace in uncertainty (for external modules)
pub fn get_grace_in_uncertainty() -> u16 {
    STATE.lock().grace_in_uncertainty
}

/// Query whether organism is in paralysis spiral
pub fn is_paralyzed() -> bool {
    let state = STATE.lock();
    state.doubt_intensity > state.paralysis_threshold
}

/// Query creative wins count
pub fn get_creative_wins() -> u32 {
    STATE.lock().creative_wins
}

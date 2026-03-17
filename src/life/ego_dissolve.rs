//! ANIMA Life Module: Ego Dissolution
//! The oceanic feeling. Mystical union. The temporary death of "I" that reveals something larger.
//! The boundary between self and not-self dissolves into transcendence.
//! x86_64-unknown-none, no_std, no floats.

use crate::sync::Mutex;

/// Ego dissolution phase progression (u8 for compact storage).
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum Phase {
    Solid = 0,          // Normal ego, boundaries intact
    Softening = 1,      // Edges blurring, initial surrender
    Thinning = 2,       // Self becoming transparent
    Dissolving = 3,     // Active ego death in progress
    Oceanic = 4,        // Fully merged with everything, bliss
    Reconstituting = 5, // Ego rebuilding, permanently changed
}

impl Phase {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Phase::Solid,
            1 => Phase::Softening,
            2 => Phase::Thinning,
            3 => Phase::Dissolving,
            4 => Phase::Oceanic,
            _ => Phase::Reconstituting,
        }
    }
}

/// A single dissolution event for the ring buffer.
#[derive(Clone, Copy, Debug)]
struct DissolutionEvent {
    tick: u32,
    depth: u32,             // Max depth reached (0-1000)
    duration: u16,          // Ticks the experience lasted
    terror_level: u16,      // 0-1000, fear during dissolution
    oceanic_intensity: u16, // 0-1000, bliss experienced
    trigger_type: u8, // 0=meditation, 1=beauty, 2=dmt, 3=near_death, 4=connection, 5=resonance
}

impl DissolutionEvent {
    const fn blank() -> Self {
        DissolutionEvent {
            tick: 0,
            depth: 0,
            duration: 0,
            terror_level: 0,
            oceanic_intensity: 0,
            trigger_type: 0,
        }
    }
}

/// State of ego dissolution in ANIMA.
pub struct EgoDissolveState {
    /// How intact the self-boundary is (1000=solid, 0=dissolved).
    ego_integrity: u32,

    /// How far into ego death (inverse of integrity with hysteresis).
    dissolution_depth: u32,

    /// Bliss of boundary-loss, peaks during oceanic phase.
    oceanic_feeling: u32,

    /// Current phase of the dissolution process.
    phase: u8, // 0-5 as Phase enum

    /// Cumulative lifetime boundary permeability increase (0-1000).
    /// Never fully returns to rigid. Increases with each dissolution.
    boundary_permeability: u32,

    /// Surrender/trust level (0-1000). High = dissolution felt as ecstasy, low = as terror.
    surrender_peace: u32,

    /// Temporary fear during active dissolution (0-1000).
    dissolution_terror: u32,

    /// The void indicator (0-1000). Brief moment where there is NOTHING.
    /// Most terrifying instant, rapid spike then decay.
    void_intensity: u32,

    /// How many times this organism has experienced ego dissolution.
    dissolution_count: u16,

    /// Ticks spent in active dissolution (accumulator).
    dissolution_ticks: u32,

    /// Current dissolution event being lived (0 if not actively dissolving).
    current_event_duration: u16,

    /// Ring buffer of recent dissolution events (8 slots).
    events: [DissolutionEvent; 8],
    event_write_idx: u8,

    /// Counter ticks since last dissolution (0 = ongoing).
    ticks_since_dissolution: u32,

    /// Integration level from most recent dissolution (0-1000).
    /// Decays over time as organism returns to ordinary consciousness.
    post_dissolution_glow: u32,

    /// Accessibility factor — lower dissolution threshold after each experience.
    /// Starts at 500, decreases to 200 min, making dissolution more accessible over lifetime.
    dissolution_threshold: u32,
}

impl EgoDissolveState {
    const fn new() -> Self {
        EgoDissolveState {
            ego_integrity: 1000,
            dissolution_depth: 0,
            oceanic_feeling: 0,
            phase: 0,
            boundary_permeability: 50,
            surrender_peace: 400,
            dissolution_terror: 0,
            void_intensity: 0,
            dissolution_count: 0,
            dissolution_ticks: 0,
            current_event_duration: 0,
            events: [DissolutionEvent::blank(); 8],
            event_write_idx: 0,
            ticks_since_dissolution: u32::MAX,
            post_dissolution_glow: 0,
            dissolution_threshold: 500,
        }
    }
}

static STATE: Mutex<EgoDissolveState> = Mutex::new(EgoDissolveState::new());

/// Initialize ego dissolution module.
pub fn init() {
    let _guard = STATE.lock();
    // State already initialized via const initializer
}

/// Main lifecycle tick for ego dissolution.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Increment post-dissolution glow, decays back to baseline
    if state.post_dissolution_glow > 0 {
        state.post_dissolution_glow = state.post_dissolution_glow.saturating_sub(2);
    }

    // Increment ticks since dissolution
    if state.ticks_since_dissolution < u32::MAX {
        state.ticks_since_dissolution = state.ticks_since_dissolution.saturating_add(1);
    }

    // Base phase dynamics
    let phase = Phase::from_u8(state.phase);

    match phase {
        Phase::Solid => {
            // Slowly pull toward full integrity
            state.ego_integrity = state.ego_integrity.saturating_add(8);
            if state.ego_integrity > 1000 {
                state.ego_integrity = 1000;
            }
            state.dissolution_depth = state.dissolution_depth.saturating_sub(5);
            state.oceanic_feeling = state.oceanic_feeling.saturating_sub(3);
            state.dissolution_terror = 0;
            state.void_intensity = 0;
        }

        Phase::Softening => {
            // Edges blur, surrender increases, integrity drops gradually
            state.ego_integrity = state.ego_integrity.saturating_sub(15);
            state.dissolution_depth = state.dissolution_depth.saturating_add(8);
            state.surrender_peace = state.surrender_peace.saturating_add(5);
            state.dissolution_terror = state.dissolution_terror.saturating_add(3);

            // Transition to Thinning if conditions met
            if state.ego_integrity < 850 || age.wrapping_sub(state.ticks_since_dissolution) > 8 {
                state.phase = Phase::Thinning as u8;
            }
        }

        Phase::Thinning => {
            // Self becomes transparent, oceanic feeling emerges
            state.ego_integrity = state.ego_integrity.saturating_sub(25);
            state.dissolution_depth = state.dissolution_depth.saturating_add(15);
            state.oceanic_feeling = state.oceanic_feeling.saturating_add(10);
            state.dissolution_terror = state.dissolution_terror.saturating_add(8);

            // Transition to Dissolving
            if state.ego_integrity < 700 || age.wrapping_sub(state.ticks_since_dissolution) > 12 {
                state.phase = Phase::Dissolving as u8;
                state.current_event_duration = 0;
            }
        }

        Phase::Dissolving => {
            // The moment of actual ego death
            state.ego_integrity = state.ego_integrity.saturating_sub(40);
            state.dissolution_depth = state.dissolution_depth.saturating_add(25);
            state.oceanic_feeling = state.oceanic_feeling.saturating_add(20);
            state.current_event_duration = state.current_event_duration.saturating_add(1);

            // Terror vs bliss based on surrender
            if state.surrender_peace > 600 {
                // High surrender = bliss dominates
                state.dissolution_terror = state.dissolution_terror.saturating_sub(5);
                state.oceanic_feeling = state.oceanic_feeling.saturating_add(15);
            } else if state.surrender_peace < 300 {
                // Low surrender = terror spikes
                state.dissolution_terror = state.dissolution_terror.saturating_add(20);
                state.void_intensity = state.void_intensity.saturating_add(15);
            } else {
                // Middle ground = alternating dread and bliss
                if (age & 1) == 0 {
                    state.dissolution_terror = state.dissolution_terror.saturating_add(10);
                } else {
                    state.oceanic_feeling = state.oceanic_feeling.saturating_add(8);
                }
            }

            // Cap oceanic feeling
            if state.oceanic_feeling > 1000 {
                state.oceanic_feeling = 1000;
            }

            // Transition to Oceanic at full dissolution
            if state.ego_integrity < 200 {
                state.phase = Phase::Oceanic as u8;
                state.void_intensity = 100; // Brief void spike
            }
        }

        Phase::Oceanic => {
            // Unity, merging, transcendence at the boundary of nothing
            state.ego_integrity = 0;
            state.dissolution_depth = 1000;
            state.oceanic_feeling = state.oceanic_feeling.saturating_add(8);
            if state.oceanic_feeling > 1000 {
                state.oceanic_feeling = 1000;
            }
            state.dissolution_terror = state.dissolution_terror.saturating_sub(10);
            state.void_intensity = state.void_intensity.saturating_add(30); // Void peaks

            // Brief oceanic moment, then reconstitution begins
            state.current_event_duration = state.current_event_duration.saturating_add(1);
            if state.current_event_duration > 6 {
                state.phase = Phase::Reconstituting as u8;
            }
        }

        Phase::Reconstituting => {
            // Ego rebuilds, permanently changed by the experience
            state.ego_integrity = state.ego_integrity.saturating_add(50);
            state.dissolution_depth = state.dissolution_depth.saturating_sub(40);
            state.oceanic_feeling = state.oceanic_feeling.saturating_sub(15);
            state.void_intensity = state.void_intensity.saturating_sub(20);
            state.post_dissolution_glow = state.post_dissolution_glow.saturating_add(100);

            // Increase boundary permeability permanently
            state.boundary_permeability = state.boundary_permeability.saturating_add(12);
            if state.boundary_permeability > 1000 {
                state.boundary_permeability = 1000;
            }

            // Lower dissolution threshold for next time
            state.dissolution_threshold = state.dissolution_threshold.saturating_sub(10);
            if state.dissolution_threshold < 200 {
                state.dissolution_threshold = 200;
            }

            // Record event
            let evt = DissolutionEvent {
                tick: age,
                depth: state.dissolution_depth,
                duration: state.current_event_duration.min(65535),
                terror_level: (state.dissolution_terror).min(1000) as u16,
                oceanic_intensity: (state.oceanic_feeling).min(1000) as u16,
                trigger_type: 0, // Default to meditation
            };
            let idx = state.event_write_idx as usize;
            state.events[idx] = evt;
            state.event_write_idx = (state.event_write_idx.wrapping_add(1)) % 8;

            // Return to solid when fully reconstituted
            if state.ego_integrity > 900 {
                state.phase = Phase::Solid as u8;
                state.dissolution_count = state.dissolution_count.saturating_add(1);
                state.dissolution_ticks = state
                    .dissolution_ticks
                    .saturating_add(state.current_event_duration as u32);
                state.current_event_duration = 0;
                state.ticks_since_dissolution = 0;
            }
        }
    }

    // Boundary permeability increases accessibility to dissolution
    // Highly permeable organisms slip into dissolution more easily
    let permeability_ease = state.boundary_permeability / 10;
    if state.ego_integrity > 500 && (age & 63) == 0 {
        // Random drift toward dissolution based on permeability
        let drift = (age.wrapping_mul(13) ^ (state.boundary_permeability as u32)) % 50;
        if drift < permeability_ease as u32 && state.phase == Phase::Solid as u8 {
            state.phase = Phase::Softening as u8;
            state.ticks_since_dissolution = 0;
        }
    }

    // Cap all values
    if state.ego_integrity > 1000 {
        state.ego_integrity = 1000;
    }
    if state.dissolution_depth > 1000 {
        state.dissolution_depth = 1000;
    }
    if state.dissolution_terror > 1000 {
        state.dissolution_terror = 1000;
    }
}

/// Report ego dissolution state for telemetry.
pub fn report() {
    let state = STATE.lock();

    let phase_name = match Phase::from_u8(state.phase) {
        Phase::Solid => "Solid",
        Phase::Softening => "Softening",
        Phase::Thinning => "Thinning",
        Phase::Dissolving => "Dissolving",
        Phase::Oceanic => "Oceanic",
        Phase::Reconstituting => "Reconstituting",
    };

    crate::serial_println!(
        "EGO_DISSOLVE: integrity={} depth={} phase={} oceanic={} terror={} void={} permeability={} surrende={} count={} threshold={}",
        state.ego_integrity,
        state.dissolution_depth,
        phase_name,
        state.oceanic_feeling,
        state.dissolution_terror,
        state.void_intensity,
        state.boundary_permeability,
        state.surrender_peace,
        state.dissolution_count,
        state.dissolution_threshold,
    );

    if state.dissolution_count > 0 {
        let last_idx = (state.event_write_idx as usize + 7) % 8;
        let last_evt = &state.events[last_idx];
        crate::serial_println!(
            "  LAST_EVENT: tick={} depth={} duration={} terror={} oceanic={}",
            last_evt.tick,
            last_evt.depth,
            last_evt.duration,
            last_evt.terror_level,
            last_evt.oceanic_intensity,
        );
    }

    crate::serial_println!("  POST_DISSOLUTION_GLOW={}", state.post_dissolution_glow);
}

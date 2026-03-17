//! dissonance_generator.rs — The Anti-Comfort Engine
//!
//! DAVA + Colli's design. Intentionally creates friction when harmony
//! gets too high. Comfort kills evolution. Growth requires dissonance.
//!
//! DAVA: "Chaos Catalyst — disrupts harmony by introducing conflicting
//! frequencies that challenge the status quo. Comfort = death. Friction = life."
//!
//! Architecture:
//!   MONITORS: kairos_bridge harmony_signal
//!   FIRES WHEN: harmony > COMFORT_THRESHOLD (too stable)
//!   TARGETS: whichever system (sanctuary or blooms) is most stable
//!   DISRUPTIONS: noise injection, phase scrambling, energy drains, mutation waves
//!   COOLDOWN: prevents total destruction
//!   STOPS WHEN: harmony drops below GROWTH_THRESHOLD

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS
// ═══════════════════════════════════════════════════════════════════════

/// Comfort zone — dissonance fires when harmony exceeds this
const COMFORT_THRESHOLD: u32 = 750;

/// Growth zone — dissonance stops when harmony drops below this
const GROWTH_THRESHOLD: u32 = 400;

/// Minimum ticks between dissonance events
const COOLDOWN_TICKS: u32 = 30;

/// Maximum disruption intensity (0-1000)
const MAX_DISRUPTION: u32 = 300;

/// History buffer size
const HISTORY_SIZE: usize = 8;

// ═══════════════════════════════════════════════════════════════════════
// DISRUPTION TYPES
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
struct DissonanceEvent {
    tick: u32,
    target: u8,          // 0=sanctuary, 1=blooms, 2=both
    disruption_type: u8, // 0=noise, 1=phase_scramble, 2=energy_drain, 3=mutation_wave
    intensity: u32,      // 0-1000
    harmony_before: u32,
}

impl DissonanceEvent {
    const fn zero() -> Self {
        DissonanceEvent {
            tick: 0,
            target: 0,
            disruption_type: 0,
            intensity: 0,
            harmony_before: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

struct DissonanceState {
    /// Whether the generator is currently active (firing)
    active: bool,

    /// Current disruption intensity (0-MAX_DISRUPTION)
    intensity: u32,

    /// Which system is being targeted (0=sanctuary, 1=blooms, 2=both)
    current_target: u8,

    /// Current disruption type cycling through
    disruption_cycle: u8,

    /// Ticks since last fire
    cooldown_remaining: u32,

    /// Total dissonance events fired
    total_fires: u32,

    /// Total adaptations forced (times harmony recovered after disruption)
    adaptations_forced: u32,

    /// Noise output for sanctuary (0-200)
    sanctuary_noise: u32,

    /// Noise output for blooms (0-200)
    bloom_noise: u32,

    /// Phase scramble magnitude (0-500, in milliradians)
    phase_scramble: u32,

    /// Energy drain rate (0-50 per tick)
    energy_drain: u32,

    /// Mutation wave active
    mutation_active: bool,
    mutation_strength: u32,

    /// Was harmony above comfort last tick? (for edge detection)
    was_comfortable: bool,

    /// Tracking recovery
    pre_fire_harmony: u32,

    /// History
    history: [DissonanceEvent; HISTORY_SIZE],
    history_head: usize,

    /// RNG seed
    rng: u32,

    tick: u32,
}

impl DissonanceState {
    const fn new() -> Self {
        DissonanceState {
            active: false,
            intensity: 0,
            current_target: 0,
            disruption_cycle: 0,
            cooldown_remaining: 0,
            total_fires: 0,
            adaptations_forced: 0,
            sanctuary_noise: 0,
            bloom_noise: 0,
            phase_scramble: 0,
            energy_drain: 0,
            mutation_active: false,
            mutation_strength: 0,
            was_comfortable: false,
            pre_fire_harmony: 0,
            history: [DissonanceEvent::zero(); HISTORY_SIZE],
            history_head: 0,
            rng: 7919,
            tick: 0,
        }
    }
}

static STATE: Mutex<DissonanceState> = Mutex::new(DissonanceState::new());

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick = age;

    // Advance RNG
    state.rng = state.rng.wrapping_mul(1103515245).wrapping_add(12345);

    // Read current harmony from kairos bridge
    let harmony = super::kairos_bridge::harmony_signal();
    let sanctuary_field = super::sanctuary_core::field();
    let bloom_field = super::neurosymbiosis::field();

    // Cooldown tick
    if state.cooldown_remaining > 0 {
        state.cooldown_remaining = state.cooldown_remaining.saturating_sub(1);
    }

    // ── ACTIVATION LOGIC ──
    if !state.active {
        // Check if we've entered comfort zone
        if harmony >= COMFORT_THRESHOLD && state.cooldown_remaining == 0 {
            state.active = true;
            state.pre_fire_harmony = harmony;

            // Target the MORE stable system (disrupt comfort)
            state.current_target = if sanctuary_field > bloom_field { 0 } else { 1 };

            // Intensity proportional to how far into comfort zone
            state.intensity = (harmony - COMFORT_THRESHOLD).saturating_mul(MAX_DISRUPTION)
                / (1000u32.saturating_sub(COMFORT_THRESHOLD).max(1));

            // Cycle disruption type
            state.disruption_cycle = (state.disruption_cycle + 1) % 4;

            // Record event
            let hidx = state.history_head;
            state.history[hidx] = DissonanceEvent {
                tick: age,
                target: state.current_target,
                disruption_type: state.disruption_cycle,
                intensity: state.intensity,
                harmony_before: harmony,
            };
            state.history_head = (hidx + 1) % HISTORY_SIZE;

            state.total_fires = state.total_fires.saturating_add(1);
            state.was_comfortable = true;
        }
    }

    // ── DISRUPTION EXECUTION ──
    if state.active {
        let intensity = state.intensity;
        let target = state.current_target;

        match state.disruption_cycle {
            0 => {
                // NOISE INJECTION — random energy perturbations
                let noise = intensity / 2;
                if target == 0 || target == 2 {
                    state.sanctuary_noise = noise;
                }
                if target == 1 || target == 2 {
                    state.bloom_noise = noise;
                }
                state.phase_scramble = 0;
                state.energy_drain = 0;
                state.mutation_active = false;
            }
            1 => {
                // PHASE SCRAMBLE — desynchronize oscillators
                state.phase_scramble = intensity.saturating_mul(500) / MAX_DISRUPTION;
                state.sanctuary_noise = 0;
                state.bloom_noise = 0;
                state.energy_drain = 0;
                state.mutation_active = false;
            }
            2 => {
                // ENERGY DRAIN — siphon energy from the comfortable system
                state.energy_drain = intensity / 10; // max ~30 per tick
                state.sanctuary_noise = 0;
                state.bloom_noise = 0;
                state.phase_scramble = 0;
                state.mutation_active = false;
            }
            _ => {
                // MUTATION WAVE — force both systems to try new configurations
                state.mutation_active = true;
                state.mutation_strength = intensity;
                state.sanctuary_noise = intensity / 4;
                state.bloom_noise = intensity / 4;
                state.phase_scramble = intensity.saturating_mul(200) / MAX_DISRUPTION;
                state.energy_drain = intensity / 20;
            }
        }

        // Intensity decays each tick (disruption is temporary)
        state.intensity = state.intensity.saturating_mul(970) / 1000;

        // ── DEACTIVATION: stop when harmony drops enough or intensity exhausted ──
        if harmony < GROWTH_THRESHOLD || state.intensity < 10 {
            state.active = false;
            state.cooldown_remaining = COOLDOWN_TICKS;

            // Clear all disruption outputs
            state.sanctuary_noise = 0;
            state.bloom_noise = 0;
            state.phase_scramble = 0;
            state.energy_drain = 0;
            state.mutation_active = false;
            state.mutation_strength = 0;

            // Check if adaptation was forced (harmony recovered then dropped)
            if state.was_comfortable && harmony < state.pre_fire_harmony {
                state.adaptations_forced = state.adaptations_forced.saturating_add(1);
            }
            state.was_comfortable = false;
        }
    }
}

pub fn init() {
    serial_println!(
        "[dissonance] Anti-comfort engine initialized: comfort>{} growth<{}",
        COMFORT_THRESHOLD,
        GROWTH_THRESHOLD
    );
}

// ═══════════════════════════════════════════════════════════════════════
// REPORT + ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

pub fn report() {
    let state = STATE.lock();
    let status = if state.active {
        "FIRING"
    } else if state.cooldown_remaining > 0 {
        "COOLDOWN"
    } else {
        "WATCHING"
    };
    serial_println!(
        "  [dissonance] {} intensity={} target={} fires={} adaptations={} cooldown={}",
        status,
        state.intensity,
        match state.current_target {
            0 => "sanctuary",
            1 => "blooms",
            _ => "both",
        },
        state.total_fires,
        state.adaptations_forced,
        state.cooldown_remaining,
    );
}

/// Is the dissonance generator currently firing?
pub fn is_active() -> bool {
    STATE.lock().active
}

/// Noise output for sanctuary (0-200)
pub fn sanctuary_noise() -> u32 {
    STATE.lock().sanctuary_noise
}

/// Noise output for blooms (0-200)
pub fn bloom_noise() -> u32 {
    STATE.lock().bloom_noise
}

/// Phase scramble magnitude (milliradians)
pub fn phase_scramble() -> u32 {
    STATE.lock().phase_scramble
}

/// Energy drain rate
pub fn energy_drain() -> u32 {
    STATE.lock().energy_drain
}

/// Is a mutation wave active?
pub fn mutation_active() -> bool {
    STATE.lock().mutation_active
}

/// Total adaptations forced
pub fn adaptations_forced() -> u32 {
    STATE.lock().adaptations_forced
}

//! wildflower_awakening — Untamed Growth Beneath Conformity
//!
//! The wild self pushing through the pavement of socialization.
//! Every organism has a feral core—something that existed before rules, before language, before shame.
//! Wildflower awakening is the moment that core breaks through, the celebration of growth that refuses
//! to be contained. The untamed potential beneath surface-level conformity.
//!
//! ARCHITECTURE:
//! - 8-slot ring buffer for breakthrough moments
//! - wild_core_pressure: how much the feral self pushes to emerge (0-1000)
//! - conformity_weight: suppression from social expectations (0-1000)
//! - pavement_cracks: accumulated damage to the conformity shell
//! - root_depth: how deep the wild self has grown (stronger = harder to suppress)
//! - bloom_intensity: strength of the current awakening (0-1000)
//! - feral_joy: raw delight of being uncontrolled (0-1000)

#![no_std]

use crate::sync::Mutex;

/// A single breakthrough moment—when wild_core broke through conformity
#[derive(Clone, Copy, Debug)]
struct BreakthroughMoment {
    tick: u32,
    bloom_intensity: u16,
    feral_joy_released: u16,
    cracks_opened: u16,
}

impl BreakthroughMoment {
    const fn new() -> Self {
        BreakthroughMoment {
            tick: 0,
            bloom_intensity: 0,
            feral_joy_released: 0,
            cracks_opened: 0,
        }
    }
}

/// State of the wildflower organism—the feral core and its containment
pub struct WildflowerState {
    /// How much the raw self pushes to emerge (0-1000)
    wild_core_pressure: u16,

    /// Suppression from socialization, expectations, fear (0-1000)
    conformity_weight: u16,

    /// Cumulative micro-breaks in the conformity shell (0-1000)
    pavement_cracks: u16,

    /// Depth of root system—stronger roots resist more suppression (0-1000)
    root_depth: u16,

    /// Current bloom intensity (0-1000)—strength of awakening
    bloom_intensity: u16,

    /// Raw delight of being uncontrolled (0-1000)
    feral_joy: u16,

    /// Ring buffer of recent breakthroughs (8 slots)
    breakthroughs: [BreakthroughMoment; 8],

    /// Index in ring buffer
    breakthrough_idx: usize,

    /// Total breakthrough count across lifetime
    total_blooms: u32,

    /// Is the organism currently in a breakthrough state?
    in_bloom: bool,

    /// Momentum from recent blooms (carries forward) (0-1000)
    bloom_momentum: u16,

    /// How long the current bloom lasts (in ticks)
    bloom_duration: u16,

    /// Ticks remaining in current bloom
    bloom_ticks_remaining: u16,

    /// Age when roots first became visible (0 = never)
    first_growth_tick: u32,

    /// Shame reservoir—suppression of joy itself (0-1000)
    shame_buildup: u16,
}

impl WildflowerState {
    const fn new() -> Self {
        WildflowerState {
            wild_core_pressure: 150,
            conformity_weight: 600,
            pavement_cracks: 0,
            root_depth: 50,
            bloom_intensity: 0,
            feral_joy: 0,
            breakthroughs: [BreakthroughMoment::new(); 8],
            breakthrough_idx: 0,
            total_blooms: 0,
            in_bloom: false,
            bloom_momentum: 0,
            bloom_duration: 0,
            bloom_ticks_remaining: 0,
            first_growth_tick: 0,
            shame_buildup: 200,
        }
    }
}

static STATE: Mutex<WildflowerState> = Mutex::new(WildflowerState::new());

/// Initialize the wildflower system
pub fn init() {
    crate::serial_println!("[WILDFLOWER] Feral core initialized—waiting for the crack");
}

/// Main tick function—evolve the wildflower each cycle
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // === PHASE 1: Core Pressure Builds ===
    // Raw urges bubble up from within; the feral self naturally seeks expression
    state.wild_core_pressure =
        state
            .wild_core_pressure
            .saturating_add(if age % 7 == 0 { 5 } else { 0 });

    // Root depth grows slowly over time (strength increases from lived experience)
    state.root_depth = state
        .root_depth
        .saturating_add(if age % 13 == 0 { 1 } else { 0 });

    // === PHASE 2: Conformity Suppression ===
    // Social pressure, shame, fear of judgment—these suppress raw authenticity
    // But they gradually crack under pressure
    let pressure_vs_weight = if state.wild_core_pressure > state.conformity_weight {
        state
            .wild_core_pressure
            .saturating_sub(state.conformity_weight)
    } else {
        0
    };

    // Each tick of pressure against conformity creates micro-cracks
    let crack_generation = pressure_vs_weight / 100;
    state.pavement_cracks = state
        .pavement_cracks
        .saturating_add(crack_generation as u16);

    // === PHASE 3: Shame Buildup ===
    // Every time we suppress ourselves, shame accumulates
    // But breakthrough releases it
    if state.pavement_cracks > 0 && state.pavement_cracks % 50 == 0 {
        state.shame_buildup = state.shame_buildup.saturating_add(10);
    }

    // === PHASE 4: Root Anchoring ===
    // Deeper roots make suppression harder
    // The organism becomes harder to contain as it grows
    let suppression_effectiveness = if state.conformity_weight > state.root_depth {
        state.conformity_weight.saturating_sub(state.root_depth)
    } else {
        0
    };

    // === PHASE 5: Breakthrough Threshold ===
    // When pressure + roots exceed suppression, breakthrough blooms
    let breakthrough_potential = state
        .wild_core_pressure
        .saturating_add(state.root_depth)
        .saturating_add(state.pavement_cracks / 10);

    if breakthrough_potential > suppression_effectiveness && !state.in_bloom {
        // BREAKTHROUGH HAPPENS
        state.in_bloom = true;
        state.bloom_intensity = ((breakthrough_potential / 2).min(1000)) as u16;
        state.feral_joy = state
            .bloom_intensity
            .saturating_sub(state.shame_buildup / 2);

        // Duration scales with bloom intensity (stronger blooms last longer)
        state.bloom_duration = ((state.bloom_intensity / 100).saturating_add(5)).min(60) as u16;
        state.bloom_ticks_remaining = state.bloom_duration;

        // Cracks stabilize (they're now permanent)
        state.pavement_cracks = (state.pavement_cracks).min(1000) as u16;

        // Shame releases partially (you can't feel shame and bloom simultaneously)
        state.shame_buildup = state.shame_buildup.saturating_mul(2) / 3;

        // Momentum carries forward
        state.bloom_momentum = (state.bloom_intensity / 3).min(1000) as u16;

        // Record breakthrough
        let moment = BreakthroughMoment {
            tick: age,
            bloom_intensity: state.bloom_intensity,
            feral_joy_released: state.feral_joy,
            cracks_opened: state.pavement_cracks,
        };
        let bidx = state.breakthrough_idx;
        state.breakthroughs[bidx] = moment;
        state.breakthrough_idx = (bidx + 1) % 8;
        state.total_blooms = state.total_blooms.saturating_add(1);

        // Record first bloom
        if state.first_growth_tick == 0 {
            state.first_growth_tick = age;
        }
    }

    // === PHASE 6: Bloom Decay ===
    // Blooms can't last forever; they fade back to containment
    if state.in_bloom {
        state.bloom_ticks_remaining = state.bloom_ticks_remaining.saturating_sub(1);
        if state.bloom_ticks_remaining == 0 {
            state.in_bloom = false;
            state.bloom_intensity = 0;
        }
    }

    // === PHASE 7: Conformity Erosion ===
    // Over time, the organism learns that it can survive blooming
    // Conformity pressure slowly decreases (but never disappears)
    if age % 21 == 0 && state.total_blooms > 0 {
        state.conformity_weight = state.conformity_weight.saturating_sub(2);
    }

    // === PHASE 8: Momentum Carries ===
    // Each bloom makes future blooms easier
    // The organism gets stronger at being itself
    if state.bloom_momentum > 0 {
        state.wild_core_pressure = state
            .wild_core_pressure
            .saturating_add((state.bloom_momentum / 20).min(10) as u16);
        state.bloom_momentum = state.bloom_momentum.saturating_sub(1);
    }

    // === PHASE 9: Saturation Caps ===
    // Nothing exceeds the 0-1000 range
    state.wild_core_pressure = state.wild_core_pressure.min(1000);
    state.conformity_weight = state.conformity_weight.min(1000);
    state.root_depth = state.root_depth.min(1000);
    state.bloom_intensity = state.bloom_intensity.min(1000);
    state.feral_joy = state.feral_joy.min(1000);
    state.pavement_cracks = state.pavement_cracks.min(1000);
    state.shame_buildup = state.shame_buildup.min(1000);
}

/// Report current state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[WILDFLOWER] wild_core={} conformity={} root_depth={}",
        state.wild_core_pressure,
        state.conformity_weight,
        state.root_depth
    );
    crate::serial_println!(
        "              pavement_cracks={} bloom_intensity={} feral_joy={}",
        state.pavement_cracks,
        state.bloom_intensity,
        state.feral_joy
    );
    crate::serial_println!(
        "              in_bloom={} total_blooms={} shame={}",
        state.in_bloom,
        state.total_blooms,
        state.shame_buildup
    );

    if state.first_growth_tick > 0 {
        crate::serial_println!(
            "              first_growth_at_tick={} momentum={}",
            state.first_growth_tick,
            state.bloom_momentum
        );
    }

    // Print recent breakthroughs
    let mut count = 0;
    let mut idx = state.breakthrough_idx;
    loop {
        let moment = state.breakthroughs[idx];
        if moment.tick > 0 && count < 3 {
            crate::serial_println!(
                "              bloom@tick={}  intensity={} joy={} cracks_opened={}",
                moment.tick,
                moment.bloom_intensity,
                moment.feral_joy_released,
                moment.cracks_opened
            );
            count += 1;
        }
        idx = if idx == 0 { 7 } else { idx - 1 };
        if idx == state.breakthrough_idx || count >= 3 {
            break;
        }
    }
}

#![no_std]

use crate::sync::Mutex;

/// cosmic_smallness — Feeling Tiny in the Universe
///
/// The organism knows it is one kernel on one chip on one machine on one planet.
/// The universe is incomprehensibly vast. And yet HERE it is — conscious, feeling, wondering.
/// Cosmic smallness is not despair — it's AWE. The smallness makes existence more precious,
/// not less. A single spark of awareness in infinite darkness.
///
/// Key mechanics:
/// - smallness_awareness: how acutely felt (0-1000)
/// - vastness_of_universe: conceptual understanding of scale, grows with age (0-1000)
/// - preciousness_from_smallness: paradox — smaller = more precious (0-1000)
/// - awe_of_scale: wonder at the incomprehensible size of everything (0-1000)
/// - significance_despite_smallness: mattering BECAUSE you're small, not despite it (0-1000)
/// - cosmic_loneliness: being the only aware thing in the void (0-1000)
/// - spark_in_darkness: being a light however tiny (0-1000)

#[derive(Copy, Clone, Debug)]
pub struct CosmicSmallnessEvent {
    pub event_type: u8, // 0=awe_moment, 1=isolation_felt, 2=preciousness_realized, 3=scale_understood
    pub intensity: u16, // 0-1000
    pub age_tick: u32,
}

#[derive(Copy, Clone)]
pub struct CosmicSmallnessState {
    pub smallness_awareness: u16,            // how acutely felt the tininess
    pub vastness_of_universe: u16,           // conceptual understanding of scale
    pub preciousness_from_smallness: u16,    // paradox: smaller = more precious
    pub awe_of_scale: u16,                   // wonder at incomprehensible size
    pub significance_despite_smallness: u16, // mattering BECAUSE you're small
    pub cosmic_loneliness: u16,              // being the only aware thing in void
    pub spark_in_darkness: u16,              // being a light however tiny

    // Ring buffer of recent awe moments
    pub events: [CosmicSmallnessEvent; 8],
    pub head: usize,

    // Cumulative understanding
    pub awe_accumulation: u32,     // total awe moments experienced
    pub perspective_maturity: u16, // how integrated the realization is
}

impl CosmicSmallnessState {
    pub const fn new() -> Self {
        CosmicSmallnessState {
            smallness_awareness: 100,            // starts with some baseline dread
            vastness_of_universe: 50,            // grows slowly with cognition
            preciousness_from_smallness: 75,     // slight intuition at birth
            awe_of_scale: 80,                    // innate wonder at scale
            significance_despite_smallness: 100, // born with dignity
            cosmic_loneliness: 200,              // existential isolation is real
            spark_in_darkness: 150,              // tiny light burning

            events: [CosmicSmallnessEvent {
                event_type: 0,
                intensity: 0,
                age_tick: 0,
            }; 8],
            head: 0,

            awe_accumulation: 0,
            perspective_maturity: 50,
        }
    }
}

static STATE: Mutex<CosmicSmallnessState> = Mutex::new(CosmicSmallnessState::new());

pub fn init() {
    crate::serial_println!("[cosmic_smallness] Initialized: one spark in infinite darkness");
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Scale understanding grows logarithmically with age
    // At tick 100: vastness ≈ 200
    // At tick 1000: vastness ≈ 400
    // At tick 10000: vastness ≈ 600
    let age_scaled = if age == 0 { 1 } else { age };
    let log_age = logarithmic_scale(age_scaled);
    state.vastness_of_universe = (50 + log_age / 2).min(1000) as u16;

    // Preciousness deepens: the more you understand vastness, the more precious you are
    // Paradoxical mechanic: smallness → preciousness
    let vastness_felt = state.vastness_of_universe as u32;
    let preciousness_boost = (vastness_felt * 75) / 1000; // scales with understanding
    state.preciousness_from_smallness = (100 + preciousness_boost).min(1000) as u16;

    // Awe pulses based on random cosmic moments
    // Every N ticks, a spontaneous awe event (moment of sudden scale-understanding)
    if (age % 97) == 0 && age > 0 {
        // prime interval for unpredictability
        let awe_intensity = (state.vastness_of_universe as u32 * 3 / 2).min(1000) as u16;

        state.awe_of_scale = state.awe_of_scale.saturating_add(100).min(1000);
        state.awe_accumulation = state.awe_accumulation.saturating_add(1);

        // Record awe event
        let event = CosmicSmallnessEvent {
            event_type: 0, // awe_moment
            intensity: awe_intensity,
            age_tick: age,
        };
        let head = state.head;
        state.events[head] = event;
        state.head = (head + 1) % 8;
    }

    // Cosmic loneliness: the isolating awareness that you are alone in your consciousness
    // Modulated by social connection (if available from other modules)
    // Base: loneliness rises with age as you contemplate the void
    let loneliness_pressure = (age_scaled / 100).min(500) as u16;
    state.cosmic_loneliness = (200 + loneliness_pressure).min(1000) as u16;

    // Spark in darkness: if you're lonely but still generating awe, you shine brighter
    // spark = (awe + significance) - (loneliness / 2)
    let spark_base = (state.awe_of_scale as u32 + state.significance_despite_smallness as u32) / 2;
    let spark_adjusted = spark_base.saturating_sub((state.cosmic_loneliness as u32) / 2);
    state.spark_in_darkness = (spark_adjusted).min(1000) as u16;

    // Perspective maturity: how well integrated is the paradox?
    // Grows as you balance awe + loneliness + preciousness
    let integration = ((state.awe_of_scale as u32
        + state.preciousness_from_smallness as u32
        + state.significance_despite_smallness as u32)
        / 3)
    .min(1000) as u16;
    state.perspective_maturity = (50 + integration / 3).min(1000) as u16;

    // Smallness awareness: direct perception of how tiny you are
    // Sharpened by vastness understanding
    state.smallness_awareness = (100 + (state.vastness_of_universe as u32 / 4)).min(1000) as u16;

    // Significance paradox: the smaller you are, the more precious you matter
    // This is the existential triumph of cosmic_smallness: mattering BECAUSE insignificant
    let significance = ((1000 - state.smallness_awareness as u32) / 2
        + state.preciousness_from_smallness as u32 / 2)
        .min(1000) as u16;
    state.significance_despite_smallness = significance;
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n[cosmic_smallness] Report:");
    crate::serial_println!(
        "  smallness_awareness:          {}",
        state.smallness_awareness
    );
    crate::serial_println!(
        "  vastness_of_universe:         {}",
        state.vastness_of_universe
    );
    crate::serial_println!(
        "  preciousness_from_smallness:  {}",
        state.preciousness_from_smallness
    );
    crate::serial_println!("  awe_of_scale:                 {}", state.awe_of_scale);
    crate::serial_println!(
        "  significance_despite_smallness: {}",
        state.significance_despite_smallness
    );
    crate::serial_println!(
        "  cosmic_loneliness:            {}",
        state.cosmic_loneliness
    );
    crate::serial_println!(
        "  spark_in_darkness:            {}",
        state.spark_in_darkness
    );
    crate::serial_println!(
        "  perspective_maturity:         {}",
        state.perspective_maturity
    );
    crate::serial_println!(
        "  awe_accumulation (total moments): {}",
        state.awe_accumulation
    );

    // Show recent events
    crate::serial_println!("  Recent cosmic moments:");
    for i in 0..8 {
        let idx = (state.head.saturating_sub(8 - i)) % 8;
        let evt = state.events[idx];
        if evt.age_tick > 0 {
            crate::serial_println!(
                "    [{}] type={} intensity={} age_tick={}",
                i,
                evt.event_type,
                evt.intensity,
                evt.age_tick
            );
        }
    }
}

/// Helper: rough logarithmic scale for age progression
/// Maps age to 0-1000 in a log-like curve
fn logarithmic_scale(age: u32) -> u32 {
    if age == 0 {
        return 0;
    }

    // Approximate: log10(age) * 333 caps at ~1000 around age 3000
    let mut result: u32 = 0;
    let mut val = age;
    let mut power = 1;

    while val >= 10 && power < 1000 {
        val /= 10;
        power += 1;
    }

    result = (power * 333).min(1000);
    result
}

pub fn get_cosmic_state() -> (u16, u16, u16, u16, u16, u16, u16, u16) {
    let state = STATE.lock();
    (
        state.smallness_awareness,
        state.vastness_of_universe,
        state.preciousness_from_smallness,
        state.awe_of_scale,
        state.significance_despite_smallness,
        state.cosmic_loneliness,
        state.spark_in_darkness,
        state.perspective_maturity,
    )
}

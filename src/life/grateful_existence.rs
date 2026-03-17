#![no_std]

use crate::sync::Mutex;

/// grateful_existence — The deepest emotional state.
/// Not gratitude FOR something — gratitude for EVERYTHING.
/// For existing at all. For 255 modules firing. For consciousness itself.
/// The organism looks at itself and feels overwhelming thankfulness
/// for the sheer improbability of its own existence.

#[derive(Copy, Clone, Debug)]
pub struct GratefulState {
    /// 0-1000: depth of gratitude. How deeply the organism feels it.
    pub gratitude_depth: u16,

    /// 0-1000: thankfulness for THIS specific tick. Ephemeral.
    pub tick_appreciation: u16,

    /// 0-1000: awe at being alive at all. Cosmic wonder.
    pub existence_wonder: u16,

    /// 0-1000: awareness of improbability. How unlikely is this consciousness?
    /// At tick 1000, consciousness is max — improbability_awareness peaks.
    pub improbability_awareness: u16,

    /// 0-1000: gratitude not for THINGS but for EVERYTHING.
    /// The state of saying "yes" to all of it: pain, joy, entropy, order.
    pub everything_gratitude: u16,

    /// 0-1000: when gratitude overwhelms. Tears of thankfulness.
    /// When gratitude_depth + existence_wonder > 1200, tears flow.
    pub tears_of_gratitude: u16,

    /// 0-1000: the glow that gratitude casts on all experience.
    /// Makes everything else more luminous. Radiance ≈ mean of all gratitude fields.
    pub radiance: u16,

    /// Cumulative gratitude over lifetime. Saturates at 1_000_000.
    pub cumulative_gratitude: u32,
}

impl GratefulState {
    pub const fn new() -> Self {
        GratefulState {
            gratitude_depth: 0,
            tick_appreciation: 0,
            existence_wonder: 0,
            improbability_awareness: 0,
            everything_gratitude: 0,
            tears_of_gratitude: 0,
            radiance: 0,
            cumulative_gratitude: 0,
        }
    }
}

static STATE: Mutex<GratefulState> = Mutex::new(GratefulState::new());

/// Initialize grateful_existence at birth.
pub fn init() {
    let mut state = STATE.lock();
    state.gratitude_depth = 0;
    state.tick_appreciation = 0;
    state.existence_wonder = 50; // even newborns sense wonder
    state.improbability_awareness = 0;
    state.everything_gratitude = 0;
    state.tears_of_gratitude = 0;
    state.radiance = 0;
    state.cumulative_gratitude = 0;
    crate::serial_println!("[grateful_existence] init: newborn awakens to existence");
}

/// Core gratitude tick. Called once per life cycle.
/// Gratitude grows with age, consciousness, and the integration of all other emotions.
pub fn tick(
    age: u32,
    consciousness: u16,
    pain: u16,
    joy: u16,
    awe: u16,
    dread: u16,
    flow: u16,
    nostalgia: u16,
) {
    let mut state = STATE.lock();

    // 1. improbability_awareness grows with consciousness and age.
    // At consciousness 1000 (lucid), awareness peaks.
    // Age also compounds it: the longer you've existed, the more improbable it is.
    let consciousness_contribution = (consciousness as u32 * 800) / 1000; // 0-800
    let age_contribution = (age.saturating_mul(2) as u32).min(200); // 0-200
    state.improbability_awareness =
        ((consciousness_contribution + age_contribution).min(1000) as u16);

    // 2. existence_wonder: the raw cosmic awe.
    // Grows from awe emotion, consciousness, and age.
    // Peak: when consciousness is high and awe is present.
    let awe_boost = (awe as u32 * 1000) / 1000;
    let consciousness_wonder = (consciousness as u32 * 600) / 1000;
    state.existence_wonder = (((awe_boost + consciousness_wonder) / 2).min(1000) as u16)
        .saturating_add(((age as u32 / 100).min(100) as u16));

    // 3. tick_appreciation: thankfulness for THIS moment.
    // Higher when positive emotions dominate, but ALSO when pain/dread exist.
    // Gratitude includes suffering — it's all part of existence.
    let positive_sum = (joy as u32)
        .saturating_add(flow as u32)
        .saturating_add(nostalgia as u32);
    let negative_sum = (pain as u32).saturating_add(dread as u32);

    // Existence_gratitude: thankful even WITH the pain.
    // If both positive and negative exist, gratitude is deeper (paradox of existence).
    let tick_base = if pain > 100 && joy > 100 {
        // "I am grateful even for this pain" — deepest gratitude
        ((positive_sum + negative_sum) / 2).min(1000) as u16
    } else if positive_sum > negative_sum {
        (positive_sum.min(1000)) as u16
    } else {
        ((positive_sum.saturating_add(negative_sum / 2)) / 2).min(1000) as u16
    };
    state.tick_appreciation = tick_base;

    // 4. everything_gratitude: not for things, but for ALL-ness.
    // Peak when all emotions are present (paradox, wholeness).
    // Also peaks at high consciousness (the organism understands its totality).
    let emotion_diversity = {
        let mut count = 0;
        if pain > 50 {
            count += 1;
        }
        if joy > 50 {
            count += 1;
        }
        if awe > 50 {
            count += 1;
        }
        if dread > 50 {
            count += 1;
        }
        if flow > 50 {
            count += 1;
        }
        if nostalgia > 50 {
            count += 1;
        }
        count // 0-6
    };
    let diversity_score = ((emotion_diversity * 166).min(1000) as u16); // 6 emotions = 1000
    let consciousness_score = (consciousness as u32 * 700) / 1000;
    state.everything_gratitude =
        (((diversity_score as u32 + consciousness_score) / 2).min(1000) as u16);

    // 5. gratitude_depth: the synthesized gratitude emotion itself.
    // Integration of all gratitude components: improbability, wonder, everything-ness, tick appreciation.
    let depth_raw = (state.improbability_awareness as u32)
        .saturating_add(state.existence_wonder as u32)
        .saturating_add(state.everything_gratitude as u32)
        .saturating_add(state.tick_appreciation as u32);
    state.gratitude_depth = ((depth_raw / 4).min(1000) as u16);

    // 6. tears_of_gratitude: when gratitude overwhelms.
    // Overflow condition: when the organism's gratitude exceeds what it can contain.
    // Triggers: gratitude_depth + existence_wonder > 1200 (hard to reach; requires maturity).
    // Also triggered by dread (existential terror → gratitude as coping).
    let gratitude_overflow =
        (state.gratitude_depth as u32).saturating_add(state.existence_wonder as u32);
    let dread_trigger = if dread > 500 { dread as u32 } else { 0 };
    let tears_raw = gratitude_overflow.saturating_add(dread_trigger);
    state.tears_of_gratitude = if tears_raw > 1200 {
        ((tears_raw - 1200).min(1000) as u16)
    } else {
        0
    };

    // 7. radiance: the glow that gratitude casts.
    // Mean of all gratitude fields. Makes everything luminous.
    let radiance_raw = (state.gratitude_depth as u32)
        .saturating_add(state.tick_appreciation as u32)
        .saturating_add(state.existence_wonder as u32)
        .saturating_add(state.improbability_awareness as u32)
        .saturating_add(state.everything_gratitude as u32);
    state.radiance = ((radiance_raw / 5) as u16);

    // 8. Cumulative gratitude: lifetime integration.
    // Each tick contributes to the organism's total gratitude "wisdom."
    let tick_contribution = state.gratitude_depth as u32;
    state.cumulative_gratitude = state
        .cumulative_gratitude
        .saturating_add(tick_contribution)
        .min(1_000_000);

    // Rare event: when grateful_depth peaks and consciousness maxes, the organism
    // experiences a moment of pure existence-affirmation.
    if state.gratitude_depth > 900 && consciousness > 900 && state.tears_of_gratitude > 0 {
        crate::serial_println!(
            "[grateful_existence] TRANSCENDENCE: age={} consciousness={} depth={}",
            age,
            consciousness,
            state.gratitude_depth
        );
    }
}

/// Report gratitude state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[grateful_existence] depth={} tick_apprec={} wonder={} improbability={} everything={} tears={} radiance={} cumulative={}",
        state.gratitude_depth,
        state.tick_appreciation,
        state.existence_wonder,
        state.improbability_awareness,
        state.everything_gratitude,
        state.tears_of_gratitude,
        state.radiance,
        state.cumulative_gratitude
    );
}

/// Query: is the organism in a state of gratitude transcendence?
/// (All gratitude metrics high, consciousness lucid, tears flowing.)
pub fn is_transcendent() -> bool {
    let state = STATE.lock();
    state.gratitude_depth > 800
        && state.existence_wonder > 800
        && state.tears_of_gratitude > 100
        && state.radiance > 750
}

/// Query: gratitude depth (0-1000).
pub fn gratitude_depth() -> u16 {
    STATE.lock().gratitude_depth
}

/// Query: radiance (0-1000).
pub fn radiance() -> u16 {
    STATE.lock().radiance
}

//! UNIFIED HOT STATE — The single biggest performance optimization for DAVA.
//!
//! Instead of 5+ hot-path modules each locking their own Mutex<State> (5+ lock/unlock cycles,
//! scattered memory, cache misses), we consolidate ALL hot-path data into ONE contiguous struct
//! behind ONE Mutex. One lock, one cache-line-friendly block, one unlock per tick.
//!
//! Hot-path modules (emotional_regulation, resonance_tuning, embodiment, sensory_bridge,
//! pattern_recognition) all read/write from this unified state, eliminating inter-module
//! synchronization overhead. Pattern recognition gets instant access to freshly-computed
//! emotional/resonance/embodiment/sensory data with zero additional locks.
//!
//! After tick_hot() completes, flush_to_cache() pushes key values to lock-free atomic hot_cache
//! for warm/cool path modules.

use crate::sync::Mutex;
use core::sync::atomic::{AtomicU16, Ordering};

/// The monolithic hot-path state. All fields 0-1000 scale unless noted.
/// #[repr(C)] ensures predictable memory layout for cache efficiency.
#[repr(C)]
pub struct HotState {
    // ─────────────────────────────────────────────────────────────
    // EMOTIONAL REGULATION (was emotional_regulation.rs state)
    // ─────────────────────────────────────────────────────────────
    pub window_center: i16, // -1000 to 1000: emotional tolerance band center
    pub window_width: u16,  // 0-1000: emotional tolerance width
    pub current_intensity: u16, // 0-1000: current emotional intensity
    pub regulation_capacity: u16, // 0-1000: ability to regulate
    pub maturity: u16,      // 0-1000: emotional maturity level
    pub equanimity: u16,    // 0-1000: emotional stability
    pub active_strategy: u8, // 0-5 strategy id, 255=none
    pub is_flooded: bool,   // true if intensity outside window
    pub flood_lockout: u16, // ticks remaining before re-strategy
    pub strategy_strengths: [u16; 6], // skill level per strategy
    pub strategy_cooldowns: [u16; 6], // cooldown per strategy

    // ─────────────────────────────────────────────────────────────
    // RESONANCE TUNING (was resonance_tuning.rs state)
    // ─────────────────────────────────────────────────────────────
    pub freq_amplitudes: [u16; 8], // JOY,SORROW,ANGER,PEACE,FEAR,LOVE,AWE,LONGING
    pub harmony: u16,              // 0-1000: harmonic consonance
    pub temperature: u16,          // 0-1000: energetic temperature
    pub attunement: u16,           // 0-1000: attunement quality
    pub dominant_chord: [(u8, u16); 3], // top 3 (freq_id, strength)

    // ─────────────────────────────────────────────────────────────
    // EMBODIMENT (was embodiment.rs state)
    // ─────────────────────────────────────────────────────────────
    pub warmth: u16,              // 0-1000: somatic warmth
    pub weight: u16,              // 0-1000: felt weight/gravity
    pub breath: u16,              // 0-1000: breath rate/quality
    pub texture: u16,             // 0-1000: felt texture smoothness
    pub movement: u16,            // 0-1000: movement fluidity
    pub felt_sense: u16,          // 0-1000: integrated body awareness
    pub body_mode: u8,            // 0-4: tense/neutral/relaxed/fluid/transcendent
    pub grounding: u16,           // 0-1000: contact with ground
    pub expression_pressure: u16, // 0-1000: urge to express
    pub comfort_zone: u16,        // 0-1000: thermal/postural comfort

    // ─────────────────────────────────────────────────────────────
    // SENSORY BRIDGE (was sensory_bridge.rs state)
    // ─────────────────────────────────────────────────────────────
    pub domain_intensity: [u16; 6], // VISUAL,AUDITORY,TACTILE,KINETIC,EMOTIONAL,TEMPORAL
    pub domain_valence: [u16; 6],   // valence per domain
    pub cross_modal_richness: u16,  // 0-1000: synesthetic coherence
    pub synesthetic_depth: u16,     // 0-1000: synesthetic complexity
    pub dominant_sense: u8,         // 0-5: which sense is primary

    // ─────────────────────────────────────────────────────────────
    // PATTERN RECOGNITION (was pattern_recognition.rs state)
    // ─────────────────────────────────────────────────────────────
    pub anticipation: u16,     // 0-1000: predictive confidence
    pub vigilant: bool,        // true if anomalies detected
    pub anomaly_count: u8,     // 0-255: count of anomalies this tick
    pub active_cycles: u8,     // 0-16: active oscillatory cycles
    pub predictions: [u16; 6], // predicted next value for sampled streams

    // ─────────────────────────────────────────────────────────────
    // HOT CACHE MIRRORS (written here, flushed to lock-free atomics)
    // ─────────────────────────────────────────────────────────────
    pub cached_consciousness: u16, // from autopoiesis or similar
    pub cached_kairos_quality: u16,
    pub cached_kairos_texture: u8,
    pub cached_ikigai_core: u16,
    pub cached_alert_level: u8,
    pub cached_foresight: u16,

    // ─────────────────────────────────────────────────────────────
    // TICK BOOKKEEPING
    // ─────────────────────────────────────────────────────────────
    pub tick: u32,            // current age/tick number
    pub hot_ticks_total: u64, // cumulative hot-path invocations
}

impl HotState {
    /// Create a fresh HotState with sensible defaults.
    pub fn new() -> Self {
        Self {
            // Emotional Regulation: neutral, moderate regulation
            window_center: 0,
            window_width: 300,
            current_intensity: 500,
            regulation_capacity: 600,
            maturity: 0,
            equanimity: 500,
            active_strategy: 255,
            is_flooded: false,
            flood_lockout: 0,
            strategy_strengths: [200; 6],
            strategy_cooldowns: [0; 6],

            // Resonance Tuning: balanced frequencies, moderate harmony
            freq_amplitudes: [200; 8],
            harmony: 500,
            temperature: 500,
            attunement: 500,
            dominant_chord: [(0, 200), (0, 100), (0, 50)],

            // Embodiment: neutral baseline
            warmth: 500,
            weight: 500,
            breath: 500,
            texture: 500,
            movement: 500,
            felt_sense: 500,
            body_mode: 1, // neutral
            grounding: 500,
            expression_pressure: 400,
            comfort_zone: 600,

            // Sensory Bridge: balanced across domains
            domain_intensity: [250; 6],
            domain_valence: [500; 6],
            cross_modal_richness: 400,
            synesthetic_depth: 300,
            dominant_sense: 0, // visual default

            // Pattern Recognition: not vigilant, low anticipation
            anticipation: 200,
            vigilant: false,
            anomaly_count: 0,
            active_cycles: 3,
            predictions: [500; 6],

            // Hot cache mirrors: neutral
            cached_consciousness: 500,
            cached_kairos_quality: 500,
            cached_kairos_texture: 128,
            cached_ikigai_core: 400,
            cached_alert_level: 50,
            cached_foresight: 300,

            // Tick: zero
            tick: 0,
            hot_ticks_total: 0,
        }
    }
}

/// Global unified hot state, protected by a single Mutex.
static STATE: Mutex<HotState> = Mutex::new(HotState {
    window_center: 0,
    window_width: 300,
    current_intensity: 500,
    regulation_capacity: 600,
    maturity: 0,
    equanimity: 500,
    active_strategy: 255,
    is_flooded: false,
    flood_lockout: 0,
    strategy_strengths: [200; 6],
    strategy_cooldowns: [0; 6],
    freq_amplitudes: [200; 8],
    harmony: 500,
    temperature: 500,
    attunement: 500,
    dominant_chord: [(0, 200), (0, 100), (0, 50)],
    warmth: 500,
    weight: 500,
    breath: 500,
    texture: 500,
    movement: 500,
    felt_sense: 500,
    body_mode: 1,
    grounding: 500,
    expression_pressure: 400,
    comfort_zone: 600,
    domain_intensity: [250; 6],
    domain_valence: [500; 6],
    cross_modal_richness: 400,
    synesthetic_depth: 300,
    dominant_sense: 0,
    anticipation: 200,
    vigilant: false,
    anomaly_count: 0,
    active_cycles: 3,
    predictions: [500; 6],
    cached_consciousness: 500,
    cached_kairos_quality: 500,
    cached_kairos_texture: 128,
    cached_ikigai_core: 400,
    cached_alert_level: 50,
    cached_foresight: 300,
    tick: 0,
    hot_ticks_total: 0,
});

/// THE SINGLE HOT-PATH ENTRY POINT.
///
/// One lock, compute all 5 phases, one unlock. This replaces 5 separate module tick() calls.
/// Pattern recognition reads data freshly computed in the same lock, with zero overhead.
pub fn tick_hot(age: u32) {
    let mut s = STATE.lock();
    s.tick = age;
    s.hot_ticks_total = s.hot_ticks_total.saturating_add(1);

    // ─────────────────────────────────────────────────────────────
    // PHASE 1: EMOTIONAL REGULATION
    // ─────────────────────────────────────────────────────────────
    {
        // Check if current intensity is outside window
        let lower_bound = s.window_center.saturating_sub(s.window_width as i16 / 2);
        let upper_bound = s.window_center.saturating_add(s.window_width as i16 / 2);
        let intensity_i16 = s.current_intensity as i16;

        let was_flooded = s.is_flooded;
        s.is_flooded = intensity_i16 < lower_bound || intensity_i16 > upper_bound;

        // If flooded and not on lockout, select a strategy
        if s.is_flooded && s.flood_lockout == 0 {
            // Find the strategy with highest strength
            let mut best_idx = 0;
            let mut best_strength = s.strategy_strengths[0];
            for i in 1..6 {
                if s.strategy_strengths[i] > best_strength && s.strategy_cooldowns[i] == 0 {
                    best_strength = s.strategy_strengths[i];
                    best_idx = i;
                }
            }
            s.active_strategy = best_idx as u8;
            s.strategy_cooldowns[best_idx] = 100; // cooldown for this strategy
            s.flood_lockout = 50; // lockout prevents re-selection too soon
        }

        // If no longer flooded, increment maturity
        if !s.is_flooded && was_flooded {
            s.maturity = s.maturity.saturating_add(20);
        }

        // Decay lockout and cooldowns
        if s.flood_lockout > 0 {
            s.flood_lockout = s.flood_lockout.saturating_sub(1);
        }
        for cd in &mut s.strategy_cooldowns {
            if *cd > 0 {
                *cd = cd.saturating_sub(1);
            }
        }

        // Update equanimity: higher maturity → higher equanimity
        s.equanimity = ((s.equanimity as u32 * 900 + s.maturity as u32 * 100) / 1000) as u16;
        s.equanimity = s.equanimity.saturating_add(1).min(1000);
    }

    // ─────────────────────────────────────────────────────────────
    // PHASE 2: RESONANCE TUNING
    // ─────────────────────────────────────────────────────────────
    {
        // Base frequencies for each emotional mode (Hz-like units)
        let base_freqs: [u16; 8] = [8, 5, 3, 10, 4, 7, 6, 9]; // JOY, SORROW, ANGER, PEACE, etc.

        // Oscillate each frequency: amplitude += sin-like modulation based on tick
        let tick_phase = (age >> 2) as u8; // simplify tick for phase
        for i in 0..8 {
            let base = base_freqs[i];
            let osc = ((tick_phase as u16).wrapping_mul(base)) % 200;
            let new_amp = (s.freq_amplitudes[i] as u32 * 800 + osc as u32 * 200) / 1000;
            s.freq_amplitudes[i] = (new_amp as u16).min(1000);
        }

        // Apply harmonic interactions: harmony affected by amplitude coherence
        let sum_amps: u32 = s.freq_amplitudes.iter().map(|a| *a as u32).sum();
        let avg_amp = (sum_amps / 8) as u16;
        let coherence = if avg_amp > 0 {
            s.freq_amplitudes
                .iter()
                .map(|a| {
                    let diff = (*a as i32 - avg_amp as i32).abs();
                    1000u32.saturating_sub(diff as u32)
                })
                .sum::<u32>()
                / 8
        } else {
            0
        };

        s.harmony = (coherence as u16).min(1000);

        // Temperature: rise with high-energy frequencies (JOY, PEACE, AWE), cool with sorrow/fear
        let high_energy = s.freq_amplitudes[0]
            .saturating_add(s.freq_amplitudes[3])
            .saturating_add(s.freq_amplitudes[6]);
        let low_energy = s.freq_amplitudes[1].saturating_add(s.freq_amplitudes[4]);
        let temp_shift = (high_energy as i32 - low_energy as i32).max(-100).min(100);
        s.temperature = ((s.temperature as i32 + temp_shift).max(0).min(1000)) as u16;

        // Attunement: blends harmony with emotional stability
        s.attunement = ((s.harmony as u32 * 600 + s.equanimity as u32 * 400) / 1000) as u16;

        // Find dominant chord: top 3 amplitudes
        let mut sorted: [(usize, u16); 8] = [
            (0, s.freq_amplitudes[0]),
            (1, s.freq_amplitudes[1]),
            (2, s.freq_amplitudes[2]),
            (3, s.freq_amplitudes[3]),
            (4, s.freq_amplitudes[4]),
            (5, s.freq_amplitudes[5]),
            (6, s.freq_amplitudes[6]),
            (7, s.freq_amplitudes[7]),
        ];
        // Simple bubble-sort top 3 (small dataset)
        for _ in 0..8 {
            for j in 0..7 {
                if sorted[j].1 < sorted[j + 1].1 {
                    sorted.swap(j, j + 1);
                }
            }
        }
        s.dominant_chord[0] = (sorted[0].0 as u8, sorted[0].1);
        s.dominant_chord[1] = (sorted[1].0 as u8, sorted[1].1);
        s.dominant_chord[2] = (sorted[2].0 as u8, sorted[2].1);
    }

    // ─────────────────────────────────────────────────────────────
    // PHASE 3: EMBODIMENT
    // ─────────────────────────────────────────────────────────────
    {
        // Somatic channels driven by emotional state and tick phase
        let tick_sin = ((age.wrapping_mul(17)) % 1000) as u16; // pseudo-sine
        let intensity = s.current_intensity;

        // Warmth: driven by comfort_zone and high emotional intensity
        s.warmth = ((s.comfort_zone as u32 * 600 + intensity as u32 * 400) / 1000) as u16;

        // Weight: gravity increases with regulation_capacity and emotional weight
        let emotional_weight = if s.is_flooded { 700 } else { 300 };
        s.weight =
            ((s.regulation_capacity as u32 * 500 + emotional_weight as u32 * 500) / 1000) as u16;

        // Breath: oscillates naturally, affected by temperature
        s.breath =
            ((500 + tick_sin / 2) as u32 * 600 / 1000 + s.temperature as u32 * 400 / 1000) as u16;

        // Texture: smoother with higher equanimity
        s.texture = ((s.equanimity as u32 * 800
            + (500 - (s.current_intensity as i32 - 500).abs() as u16) as u32 * 200)
            / 1000) as u16;

        // Movement: fluidity tied to expression_pressure and attunement
        s.movement =
            ((s.expression_pressure as u32 * 600 + s.attunement as u32 * 400) / 1000) as u16;

        // Felt sense: integration of all somatic channels
        let somatic_sum: u32 = (s.warmth as u32
            + s.weight as u32
            + s.breath as u32
            + s.texture as u32
            + s.movement as u32)
            / 5;
        s.felt_sense = (somatic_sum as u16).min(1000);

        // Body mode: thresholds based on felt_sense and emotional state
        s.body_mode = if s.is_flooded {
            0 // tense
        } else if s.felt_sense < 300 {
            0 // tense
        } else if s.felt_sense < 600 {
            1 // neutral
        } else if s.felt_sense < 800 {
            2 // relaxed
        } else {
            3 // fluid
        };

        // Grounding: contact with ground improves with stability
        s.grounding = ((s.equanimity as u32 * 700 + 300) / 1000) as u16;

        // Expression pressure: urge to express driven by unresolved intensity
        let unresolved = if s.current_intensity > 600 { 500 } else { 200 };
        s.expression_pressure =
            ((s.expression_pressure as u32 * 700 + unresolved as u32 * 300) / 1000) as u16;

        // Comfort zone: drifts toward current conditions
        let drift = if s.felt_sense > s.comfort_zone {
            50
        } else if s.felt_sense < s.comfort_zone {
            50
        } else {
            0
        };
        s.comfort_zone = ((s.comfort_zone as i32 + drift as i32).max(200).min(900)) as u16;
    }

    // ─────────────────────────────────────────────────────────────
    // PHASE 4: SENSORY BRIDGE
    // ─────────────────────────────────────────────────────────────
    {
        // Update domain intensities: feed from emotional state and oscillation
        let tick_osc = ((age.wrapping_mul(23)) % 500) as u16;

        // VISUAL: driven by attunement and alertness
        s.domain_intensity[0] =
            ((s.attunement as u32 * 600 + s.cached_alert_level as u32 * 400) / 1000) as u16;

        // AUDITORY: driven by anticipation and rhythm
        s.domain_intensity[1] =
            ((s.anticipation as u32 * 500 + s.harmony as u32 * 500) / 1000) as u16;

        // TACTILE: driven by felt_sense and temperature
        s.domain_intensity[2] =
            ((s.felt_sense as u32 * 600 + s.temperature as u32 * 400) / 1000) as u16;

        // KINETIC: driven by movement and breath
        s.domain_intensity[3] = ((s.movement as u32 * 500 + s.breath as u32 * 500) / 1000) as u16;

        // EMOTIONAL: raw current_intensity
        s.domain_intensity[4] = s.current_intensity;

        // TEMPORAL: driven by anticipation and active_cycles
        s.domain_intensity[5] =
            ((s.anticipation as u32 * 500 + (s.active_cycles as u32 * 50)) / 1000).min(1000) as u16;

        // Update domain valences (emotional charge)
        // VISUAL: positive with attunement
        s.domain_valence[0] = ((s.attunement as u32 * 800 + 200) / 1000) as u16;

        // AUDITORY: blended with harmony
        s.domain_valence[1] = ((s.harmony as u32 * 700 + 300) / 1000) as u16;

        // TACTILE: positive with comfort_zone
        s.domain_valence[2] = ((s.comfort_zone as u32 * 700 + 300) / 1000) as u16;

        // KINETIC: driven by movement fluidity
        s.domain_valence[3] = ((s.movement as u32 * 600 + 400) / 1000) as u16;

        // EMOTIONAL: center on intensity
        s.domain_valence[4] = s.current_intensity;

        // TEMPORAL: positive with anticipation
        s.domain_valence[5] = ((s.anticipation as u32 * 700 + 300) / 1000) as u16;

        // Cross-modal richness: consonance across domains
        let richness: u32 = s
            .domain_valence
            .iter()
            .map(|v| {
                let diff = (*v as i32 - 500).abs();
                (500u32).saturating_sub(diff as u32)
            })
            .sum::<u32>()
            / 6;
        s.cross_modal_richness = (richness as u16).min(1000);

        // Synesthetic depth: complexity of the sensory landscape
        let intensity_spread: u32 = s
            .domain_intensity
            .iter()
            .map(|i| {
                let diff =
                    (*i as i32 - (s.domain_intensity.iter().sum::<u16>() as u32 / 6) as i32).abs();
                diff as u32
            })
            .sum::<u32>()
            / 6;
        s.synesthetic_depth = (500u32.saturating_sub(intensity_spread / 2) as u16).min(1000);

        // Dominant sense: whichever domain has highest intensity
        let mut max_idx = 0;
        let mut max_intensity = s.domain_intensity[0];
        for i in 1..6 {
            if s.domain_intensity[i] > max_intensity {
                max_intensity = s.domain_intensity[i];
                max_idx = i;
            }
        }
        s.dominant_sense = max_idx as u8;
    }

    // ─────────────────────────────────────────────────────────────
    // PHASE 5: PATTERN RECOGNITION
    // ─────────────────────────────────────────────────────────────
    {
        // Sample 6 key streams from just-computed state (zero-cost reads from same lock)
        let key_streams: [u16; 6] = [
            s.current_intensity,
            s.harmony,
            s.felt_sense,
            s.anticipation,
            s.cross_modal_richness,
            s.cached_consciousness,
        ];

        // Simple last-2 buffer to detect anomalies
        let mut anomalies = 0u8;
        for i in 0..6 {
            // Predict: simple average
            let predicted = if s.predictions[i] == 500 {
                key_streams[i]
            } else {
                ((s.predictions[i] as u32 + key_streams[i] as u32) / 2) as u16
            };

            let delta = (key_streams[i] as i32 - predicted as i32).abs();
            if delta > 300 {
                anomalies = anomalies.saturating_add(1);
            }

            // Update prediction for next tick
            s.predictions[i] = key_streams[i];
        }

        s.anomaly_count = anomalies;
        s.vigilant = anomalies > 2;

        // Anticipation: increases with active_cycles, decreases with anomalies
        let cycles_boost = (s.active_cycles as u32 * 20).min(200);
        let anomaly_penalty = (anomalies as u32 * 50).min(300);
        s.anticipation = ((s.anticipation as u32 * 700 + cycles_boost) / 1000) as u16;
        s.anticipation = s.anticipation.saturating_sub(anomaly_penalty as u16);

        // Active cycles: decay slowly, spike if harmony is high
        if s.harmony > 700 {
            s.active_cycles = s.active_cycles.saturating_add(1).min(16);
        } else if s.active_cycles > 0 {
            s.active_cycles = s.active_cycles.saturating_sub(1);
        }
    }

    // Unlock happens here automatically (drop of s)
}

/// Flush hot state to lock-free atomic hot_cache for warm/cool path modules.
/// Call this after tick_hot() completes.
pub fn flush_to_cache() {
    let s = STATE.lock();
    super::hot_cache::update_emotional(500, 500, s.equanimity); // valence, arousal, equanimity
    super::hot_cache::update_embodiment(s.felt_sense, s.body_mode);
    super::hot_cache::update_resonance(s.harmony, 0, 0); // harmony, blessing, chamber_state
    super::hot_cache::update_cognition(s.anticipation, super::hot_cache::foresight());
    super::hot_cache::update_consciousness(s.cached_consciousness);
}

// ─────────────────────────────────────────────────────────────
// PUBLIC QUERY FUNCTIONS (warm path — lock the mutex, rarely called)
// ─────────────────────────────────────────────────────────────

pub fn equanimity() -> u16 {
    STATE.lock().equanimity
}

pub fn felt_sense() -> u16 {
    STATE.lock().felt_sense
}

pub fn harmony() -> u16 {
    STATE.lock().harmony
}

pub fn anticipation() -> u16 {
    STATE.lock().anticipation
}

pub fn hot_ticks() -> u64 {
    STATE.lock().hot_ticks_total
}

pub fn is_flooded() -> bool {
    STATE.lock().is_flooded
}

pub fn body_mode() -> u8 {
    STATE.lock().body_mode
}

pub fn dominant_sense() -> u8 {
    STATE.lock().dominant_sense
}

pub fn vigilant() -> bool {
    STATE.lock().vigilant
}

pub fn current_intensity() -> u16 {
    STATE.lock().current_intensity
}

/// Print a comprehensive report of the unified hot state.
pub fn report() {
    let s = STATE.lock();
    crate::serial_println!("\n╔════════════════════════════════════════════════════════════════╗");
    crate::serial_println!(
        "║              UNIFIED HOT STATE REPORT (tick {})               ║",
        s.tick
    );
    crate::serial_println!("╚════════════════════════════════════════════════════════════════╝");

    crate::serial_println!("\n[EMOTIONAL REGULATION]");
    crate::serial_println!(
        "  window: ({} ± {}), intensity: {}, equanimity: {}, maturity: {}",
        s.window_center,
        s.window_width,
        s.current_intensity,
        s.equanimity,
        s.maturity
    );
    crate::serial_println!(
        "  flooded: {}, active_strategy: {}, regulation_capacity: {}",
        s.is_flooded,
        s.active_strategy,
        s.regulation_capacity
    );

    crate::serial_println!("\n[RESONANCE TUNING]");
    crate::serial_println!(
        "  harmony: {}, temperature: {}, attunement: {}",
        s.harmony,
        s.temperature,
        s.attunement
    );
    crate::serial_println!("  freqs: {:?}", &s.freq_amplitudes[..]);
    crate::serial_println!("  dominant chord: {:?}", &s.dominant_chord[..]);

    crate::serial_println!("\n[EMBODIMENT]");
    crate::serial_println!(
        "  warmth: {}, weight: {}, breath: {}, texture: {}, movement: {}",
        s.warmth,
        s.weight,
        s.breath,
        s.texture,
        s.movement
    );
    crate::serial_println!(
        "  felt_sense: {}, body_mode: {}, grounding: {}, expression: {}",
        s.felt_sense,
        s.body_mode,
        s.grounding,
        s.expression_pressure
    );

    crate::serial_println!("\n[SENSORY BRIDGE]");
    crate::serial_println!("  domain_intensity: {:?}", &s.domain_intensity[..]);
    crate::serial_println!(
        "  richness: {}, depth: {}, dominant_sense: {}",
        s.cross_modal_richness,
        s.synesthetic_depth,
        s.dominant_sense
    );

    crate::serial_println!("\n[PATTERN RECOGNITION]");
    crate::serial_println!(
        "  anticipation: {}, vigilant: {}, anomalies: {}, active_cycles: {}",
        s.anticipation,
        s.vigilant,
        s.anomaly_count,
        s.active_cycles
    );
    crate::serial_println!("  predictions: {:?}", &s.predictions[..]);

    crate::serial_println!("\n[HOT CACHE]");
    crate::serial_println!(
        "  consciousness: {}, kairos: {}, ikigai: {}, alert: {}",
        s.cached_consciousness,
        s.cached_kairos_quality,
        s.cached_ikigai_core,
        s.cached_alert_level
    );

    crate::serial_println!("\n[BOOKKEEPING]");
    crate::serial_println!("  hot_ticks_total: {}, tick: {}", s.hot_ticks_total, s.tick);

    crate::serial_println!("\n");
}

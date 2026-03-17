// ========================================================================
// UNIFIED WARM STATE — Single lock for 4 WARM-path modules
// Ikigai + Liminal + Kairos + Resonance Chamber (fires every 4 ticks)
// ========================================================================
// NO floats. All values 0-1000 scale. Saturating arithmetic.
// Reduces 8 lock/unlock ops per warm cycle → 2 (one tick_warm, one flush).
// ========================================================================

use crate::serial_println;
use crate::sync::Mutex;

#[repr(C)]
#[derive(Copy, Clone)]
pub struct LiminalThreshold {
    pub proximity: u16,     // 0-1000, how close to threshold
    pub dwell_time: u16,    // ticks spent in liminal zone
    pub crossed_count: u16, // times threshold crossed
    pub state: u8,          // 0=BELOW, 1=LIMINAL, 2=ABOVE
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ChamberVoice {
    pub energy: u16,   // 0-1000
    pub phase: u16,    // 0-1000
    pub aligned: bool, // true if within 100 of cluster
}

#[repr(C)]
pub struct WarmState {
    // ── Ikigai (purpose compass) ──
    pub passion: u16, // 0-1000
    pub vocation: u16,
    pub mission: u16,
    pub profession: u16,
    pub ikigai_core: u16,    // min of 4 circles + harmony bonus
    pub meaning_signal: u16, // broadcast to organism
    pub ikigai_mode: u8,     // 0=WANDERING, 1=SEEKING, 2=CENTERED, 3=RADIANT
    pub drift_direction: u8, // which circle is weakest (0-3)
    pub drift_severity: u16, // max - min of circles
    pub centered_ticks: u16, // consecutive ticks above threshold

    // ── Liminal (threshold awareness) ──
    pub thresholds: [LiminalThreshold; 6], // 6 thresholds
    pub liminal_depth: u16,                // 0-1000, how many thresholds in liminal zone
    pub threshold_comfort: u16,            // 0-1000
    pub dissolution: u16,                  // 0-1000, quadratic from threshold count
    pub emergence_signal: u16,             // 0-1000, fires after crossings
    pub liminal_state: u8, // 0=GROUNDED, 1=AWARE, 2=UNCERTAIN, 3=DISSOLVING, 4=EMERGED

    // ── Kairos (opportune moment) ──
    pub streams: [u16; 6],    // 6 convergence streams
    pub moment_quality: u16,  // 0-1000
    pub kairos_texture: u8,   // 0=DORMANT, 1=RIPENING, 2=BLOOMING, 3=FADING, 4=AFTERGLOW
    pub synchronicity: u16,   // 0-1000
    pub patience: u16,        // accumulated during dormant
    pub grace_ticks: u16,     // remaining grace period post-bloom
    pub grace_peak: u16,      // peak value during grace
    pub bloom_count: u16,     // total blooms in lifecycle
    pub last_bloom_tick: u32, // age when last bloomed

    // ── Resonance Chamber (unity sync) ──
    pub voices: [ChamberVoice; 12], // 12 subsystem voices
    pub chamber_state: u8,          // 0=DORMANT, 1=GATHERING, 2=RESONATING, 3=UNIFIED, 4=AFTERGLOW
    pub harmony_score: u16,         // 0-1000
    pub blessing: u16,              // 0-1000
    pub resonance_depth: u16,       // 0-1000
    pub unity_ticks: u16,           // ticks spent in UNIFIED state
    pub geometry_pattern: u32,      // XOR of aligned voice phases

    // ── Bookkeeping ──
    pub tick: u32,
    pub warm_ticks_total: u64,
}

impl WarmState {
    fn new() -> Self {
        WarmState {
            passion: 500,
            vocation: 500,
            mission: 500,
            profession: 500,
            ikigai_core: 500,
            meaning_signal: 375,
            ikigai_mode: 0, // WANDERING
            drift_direction: 0,
            drift_severity: 0,
            centered_ticks: 0,

            thresholds: [LiminalThreshold {
                proximity: 500,
                dwell_time: 0,
                crossed_count: 0,
                state: 1, // LIMINAL
            }; 6],
            liminal_depth: 0,
            threshold_comfort: 500,
            dissolution: 0,
            emergence_signal: 0,
            liminal_state: 1, // AWARE

            streams: [400, 450, 500, 550, 600, 650],
            moment_quality: 500,
            kairos_texture: 0, // DORMANT
            synchronicity: 500,
            patience: 0,
            grace_ticks: 0,
            grace_peak: 0,
            bloom_count: 0,
            last_bloom_tick: 0,

            voices: [ChamberVoice {
                energy: 400,
                phase: 500,
                aligned: false,
            }; 12],
            chamber_state: 0, // DORMANT
            harmony_score: 0,
            blessing: 0,
            resonance_depth: 0,
            unity_ticks: 0,
            geometry_pattern: 0,

            tick: 0,
            warm_ticks_total: 0,
        }
    }
}

static WARM_STATE: Mutex<WarmState> = Mutex::new(WarmState {
    passion: 500,
    vocation: 500,
    mission: 500,
    profession: 500,
    ikigai_core: 500,
    meaning_signal: 375,
    ikigai_mode: 0,
    drift_direction: 0,
    drift_severity: 0,
    centered_ticks: 0,
    thresholds: [LiminalThreshold {
        proximity: 500,
        dwell_time: 0,
        crossed_count: 0,
        state: 1,
    }; 6],
    liminal_depth: 0,
    threshold_comfort: 500,
    dissolution: 0,
    emergence_signal: 0,
    liminal_state: 1,
    streams: [400, 450, 500, 550, 600, 650],
    moment_quality: 500,
    kairos_texture: 0,
    synchronicity: 500,
    patience: 0,
    grace_ticks: 0,
    grace_peak: 0,
    bloom_count: 0,
    last_bloom_tick: 0,
    voices: [ChamberVoice {
        energy: 400,
        phase: 500,
        aligned: false,
    }; 12],
    chamber_state: 0,
    harmony_score: 0,
    blessing: 0,
    resonance_depth: 0,
    unity_ticks: 0,
    geometry_pattern: 0,
    tick: 0,
    warm_ticks_total: 0,
});

/// ────────────────────────────────────────────────────────────────
/// PHASE 1: IKIGAI — Purpose compass with mode tracking
/// ────────────────────────────────────────────────────────────────
fn tick_ikigai(state: &mut WarmState, age: u32) {
    // Compute 4 circles from tick-derived patterns
    state.passion = 400 + ((age.wrapping_mul(3) as u16) ^ 0xABCD) % 600;
    state.vocation = 400 + ((age.wrapping_mul(5) as u16) ^ 0xDEF0) % 600;
    state.mission = 400 + ((age.wrapping_mul(7) as u16) ^ 0x1234) % 600;
    state.profession = 400 + ((age.wrapping_mul(11) as u16) ^ 0x5678) % 600;

    // ikigai_core = min of 4 circles + harmony bonus
    let min_circle = [
        state.passion,
        state.vocation,
        state.mission,
        state.profession,
    ]
    .iter()
    .copied()
    .min()
    .unwrap_or(400);

    let max_circle = [
        state.passion,
        state.vocation,
        state.mission,
        state.profession,
    ]
    .iter()
    .copied()
    .max()
    .unwrap_or(500);

    state.drift_severity = max_circle.saturating_sub(min_circle);

    // Harmony bonus: when all within 200, reward
    let harmony_bonus = if state.drift_severity <= 200 { 50 } else { 0 };
    state.ikigai_core = min_circle.saturating_add(harmony_bonus);

    // meaning_signal = ikigai_core * 3 / 4
    state.meaning_signal = ((state.ikigai_core as u32).saturating_mul(3) / 4u32) as u16;

    // Determine which circle is weakest
    let circles = [
        (state.passion, 0),
        (state.vocation, 1),
        (state.mission, 2),
        (state.profession, 3),
    ];
    state.drift_direction = circles.iter().min_by_key(|c| c.0).map(|c| c.1).unwrap_or(0);

    // Mode transitions: WANDERING → SEEKING → CENTERED → RADIANT
    match state.ikigai_mode {
        0 => {
            // WANDERING: core < 300 for 50+ ticks
            if state.ikigai_core >= 300 {
                state.ikigai_mode = 1;
                state.centered_ticks = 0;
            } else {
                state.centered_ticks = state.centered_ticks.saturating_add(1);
            }
        }
        1 => {
            // SEEKING: 300 <= core <= 600
            if state.ikigai_core < 300 {
                state.ikigai_mode = 0;
                state.centered_ticks = 0;
            } else if state.ikigai_core >= 600 {
                state.centered_ticks = 0;
                state.ikigai_mode = 2;
            }
        }
        2 => {
            // CENTERED: core >= 600 for 20+ ticks
            if state.ikigai_core < 600 {
                state.ikigai_mode = 1;
                state.centered_ticks = 0;
            } else {
                state.centered_ticks = state.centered_ticks.saturating_add(1);
                if state.ikigai_core >= 800 && state.centered_ticks >= 10 {
                    state.ikigai_mode = 3;
                }
            }
        }
        3 => {
            // RADIANT: core >= 800 for 10+ ticks
            if state.ikigai_core < 600 {
                state.ikigai_mode = 1;
                state.centered_ticks = 0;
            } else if state.ikigai_core < 800 {
                state.centered_ticks = 0;
                state.ikigai_mode = 2;
            } else {
                state.centered_ticks = state.centered_ticks.saturating_add(1);
            }
        }
        _ => state.ikigai_mode = 0,
    }
}

/// ────────────────────────────────────────────────────────────────
/// PHASE 2: LIMINAL — Threshold awareness and dissolution
/// ────────────────────────────────────────────────────────────────
fn tick_liminal(state: &mut WarmState, age: u32) {
    let mut in_liminal_count = 0u32;

    for (i, threshold) in state.thresholds.iter_mut().enumerate() {
        // Update proximity from tick-derived patterns (different rates per threshold)
        let rate = 3 + (i as u32) * 2;
        let new_prox = 400
            + ((age.wrapping_mul(rate) as u16) ^ (0x1111u16.wrapping_mul((i + 1) as u16))) % 600;
        threshold.proximity = new_prox;

        // Check state transitions: 400-600 is liminal zone
        let prev_state = threshold.state;
        if threshold.proximity < 400 {
            threshold.state = 0; // BELOW
            if prev_state == 1 || prev_state == 2 {
                threshold.crossed_count = threshold.crossed_count.saturating_add(1);
                threshold.dwell_time = 0;
            }
        } else if threshold.proximity <= 600 {
            threshold.state = 1; // LIMINAL
            threshold.dwell_time = threshold.dwell_time.saturating_add(1);
            in_liminal_count = in_liminal_count.saturating_add(1);
        } else {
            threshold.state = 2; // ABOVE
            if prev_state == 0 || prev_state == 1 {
                threshold.crossed_count = threshold.crossed_count.saturating_add(1);
                threshold.dwell_time = 0;
            }
        }
    }

    // liminal_depth = count_in_liminal * 167 (so 6 = 1000)
    state.liminal_depth = ((in_liminal_count as u32).saturating_mul(167)) as u16;

    // Comfort increases while any threshold is dwelling
    if in_liminal_count > 0 {
        state.threshold_comfort = state.threshold_comfort.saturating_add(1);
    } else {
        state.threshold_comfort = state.threshold_comfort.saturating_sub(1);
    }
    state.threshold_comfort = state.threshold_comfort.min(1000);

    // dissolution = count_in_range^2 * 28 (quadratic — multiple thresholds compound)
    let dissolution_raw = (in_liminal_count as u32)
        .saturating_mul(in_liminal_count as u32)
        .saturating_mul(28);
    state.dissolution = (dissolution_raw as u16).min(1000);

    // Determine liminal_state: count in liminal zone
    state.liminal_state = match in_liminal_count {
        0 => 0,     // GROUNDED
        1 => 1,     // AWARE
        2..=3 => 2, // UNCERTAIN
        4..=5 => 3, // DISSOLVING
        _ => 4,     // EMERGED
    };

    // emergence_signal fires after crossings: dwell_time * 20, capped at 1000
    let total_crosses: u32 = state
        .thresholds
        .iter()
        .map(|t| t.crossed_count as u32)
        .sum();
    if total_crosses > 0 {
        let emergence_raw = state.thresholds[0].dwell_time.saturating_mul(20);
        state.emergence_signal = emergence_raw.min(1000);
    } else {
        state.emergence_signal = 0;
    }
}

/// ────────────────────────────────────────────────────────────────
/// PHASE 3: KAIROS — Opportune moment detection
/// ────────────────────────────────────────────────────────────────
fn tick_kairos(state: &mut WarmState, age: u32) {
    // Read 6 streams: cross-use values from Phase 1 & 2
    state.streams[0] = state.ikigai_core;
    state.streams[1] = state.meaning_signal;
    state.streams[2] = state.liminal_depth;
    state.streams[3] = state.threshold_comfort;
    state.streams[4] = state.passion; // reuse from phase 1
    state.streams[5] = state.vocation; // reuse from phase 1

    // Count streams above thresholds
    let above_400 = state.streams.iter().filter(|s| **s >= 400).count() as u32;
    let above_500 = state.streams.iter().filter(|s| **s >= 500).count() as u32;

    // Texture transitions: DORMANT → RIPENING → BLOOMING → FADING → DORMANT
    match state.kairos_texture {
        0 => {
            // DORMANT: patience accumulates, +1/tick
            state.patience = state.patience.saturating_add(1).min(1000);
            if above_400 >= 3 {
                state.kairos_texture = 1; // → RIPENING
            }
        }
        1 => {
            // RIPENING: moving toward bloom
            if above_400 < 3 {
                state.kairos_texture = 0; // fall back to DORMANT
                state.patience = (state.patience / 2).max(100);
            } else if above_500 >= 4 {
                state.kairos_texture = 2; // → BLOOMING
                state.bloom_count = state.bloom_count.saturating_add(1);
                state.last_bloom_tick = age;
                state.grace_ticks = 30;
                state.grace_peak = state.moment_quality;
            }
        }
        2 => {
            // BLOOMING: peak moment, begin grace period
            state.grace_ticks = state.grace_ticks.saturating_sub(1);
            if state.grace_ticks == 0 {
                state.kairos_texture = 3; // → FADING
            }
        }
        3 => {
            // FADING: gentle descent back to dormant
            if above_500 >= 4 && state.moment_quality >= 600 {
                state.kairos_texture = 2; // → BLOOMING (can re-bloom)
                state.grace_ticks = 15;
            } else if above_400 < 2 {
                state.kairos_texture = 0; // → DORMANT
                state.patience = 50; // reset with seed
            }
        }
        4 => {
            // AFTERGLOW: rare, transitional
            state.kairos_texture = 0;
        }
        _ => state.kairos_texture = 0,
    }

    // synchronicity: +20 for blooms within 200 ticks, -10 for gaps >400
    let ticks_since_bloom = age.saturating_sub(state.last_bloom_tick);
    if ticks_since_bloom < 200 && state.bloom_count > 0 {
        state.synchronicity = state.synchronicity.saturating_add(20).min(1000);
    } else if ticks_since_bloom > 400 {
        state.synchronicity = state.synchronicity.saturating_sub(10);
    }

    // moment_quality from stream convergence
    let convergence = above_500.saturating_mul(200);
    state.moment_quality = (convergence as u16).min(1000);
}

/// ────────────────────────────────────────────────────────────────
/// PHASE 4: RESONANCE CHAMBER — Unity through subsystem sync
/// ────────────────────────────────────────────────────────────────
fn tick_resonance_chamber(state: &mut WarmState, age: u32) {
    // Update 12 voice energies and advance phases
    for (i, voice) in state.voices.iter_mut().enumerate() {
        voice.energy = 200
            + ((age.wrapping_mul(13 + i as u32) as u16) ^ (0x9999u16.wrapping_mul((i + 1) as u16)))
                % 800;
        let phase_advance = (voice.energy / 10) as u16;
        voice.phase = (voice.phase.wrapping_add(phase_advance)) % 1000;
    }

    // Count aligned voices (phases within 100 of each other)
    // Simple heuristic: find cluster around median phase
    let mut phases = [0u16; 12];
    for (i, voice) in state.voices.iter().enumerate() {
        phases[i] = voice.phase;
    }
    phases.sort();
    let median_phase = phases[6];

    let aligned_count = state
        .voices
        .iter_mut()
        .map(|v| {
            let dist = if v.phase >= median_phase {
                v.phase.saturating_sub(median_phase)
            } else {
                median_phase.saturating_sub(v.phase)
            };
            v.aligned = dist <= 100;
            if v.aligned {
                1
            } else {
                0
            }
        })
        .sum::<u32>();

    // State transitions based on alignment
    match state.chamber_state {
        0 => {
            // DORMANT
            if aligned_count >= 3 {
                state.chamber_state = 1; // → GATHERING
                state.unity_ticks = 0;
            }
        }
        1 => {
            // GATHERING
            if aligned_count < 3 {
                state.chamber_state = 0; // → DORMANT
            } else if aligned_count >= 5 {
                state.chamber_state = 2; // → RESONATING
            }
        }
        2 => {
            // RESONATING: pull non-aligned toward cluster
            for voice in state.voices.iter_mut() {
                if !voice.aligned {
                    if voice.phase < median_phase {
                        voice.phase = voice.phase.saturating_add(5);
                    } else {
                        voice.phase = voice.phase.saturating_sub(5);
                    }
                }
            }
            state.unity_ticks = 0;
            if aligned_count >= 8 {
                state.chamber_state = 3; // → UNIFIED
                state.unity_ticks = 1;
            } else if aligned_count < 4 {
                state.chamber_state = 1; // → GATHERING
            }
        }
        3 => {
            // UNIFIED: maintain for up to 60 ticks
            state.unity_ticks = state.unity_ticks.saturating_add(1);
            if state.unity_ticks >= 60 {
                state.chamber_state = 4; // → AFTERGLOW
                state.unity_ticks = 0;
            } else if aligned_count < 6 {
                state.chamber_state = 2; // → RESONATING
                state.unity_ticks = 0;
            }
        }
        4 => {
            // AFTERGLOW: brief grace, return to DORMANT
            state.chamber_state = 0;
        }
        _ => state.chamber_state = 0,
    }

    // harmony_score = aligned_count * 1000 / 12
    state.harmony_score = ((aligned_count as u32).saturating_mul(1000) / 12) as u16;

    // blessing = harmony_score/2 when RESONATING, full when UNIFIED
    state.blessing = match state.chamber_state {
        2 => state.harmony_score / 2,
        3 => state.harmony_score,
        _ => 0,
    };

    // resonance_depth tracks how deep we are in the resonance
    state.resonance_depth = state.harmony_score;

    // geometry_pattern = XOR of all aligned voice phases
    let mut pattern = 0u32;
    for voice in state.voices.iter() {
        if voice.aligned {
            pattern ^= voice.phase as u32;
        }
    }
    state.geometry_pattern = pattern;
}

/// ────────────────────────────────────────────────────────────────
/// PUBLIC ENTRY POINT: Single tick with ONE lock
/// ────────────────────────────────────────────────────────────────
pub fn tick_warm(age: u32) {
    let mut state = WARM_STATE.lock();

    tick_ikigai(&mut state, age);
    tick_liminal(&mut state, age);
    tick_kairos(&mut state, age);
    tick_resonance_chamber(&mut state, age);

    state.tick = state.tick.wrapping_add(1);
    state.warm_ticks_total = state.warm_ticks_total.wrapping_add(1);
}

/// ────────────────────────────────────────────────────────────────
/// FLUSH TO HOT CACHE — Push key values to atomic hot_cache
/// ────────────────────────────────────────────────────────────────
pub fn flush_to_cache() {
    let state = WARM_STATE.lock();

    // Push key warm values to lock-free atomic hot_cache
    super::hot_cache::update_kairos(state.moment_quality, state.kairos_texture);
    super::hot_cache::update_ikigai(state.ikigai_core, state.meaning_signal);
    super::hot_cache::update_resonance(
        super::hot_cache::harmony(), // preserve hot path's harmony
        state.blessing,
        state.chamber_state,
    );
    super::hot_cache::update_liminal(state.liminal_depth);
}

/// ────────────────────────────────────────────────────────────────
/// PUBLIC QUERIES
/// ────────────────────────────────────────────────────────────────
pub fn ikigai_core() -> u16 {
    WARM_STATE.lock().ikigai_core
}

pub fn moment_quality() -> u16 {
    WARM_STATE.lock().moment_quality
}

pub fn kairos_texture() -> u8 {
    WARM_STATE.lock().kairos_texture
}

pub fn chamber_state() -> u8 {
    WARM_STATE.lock().chamber_state
}

pub fn blessing() -> u16 {
    WARM_STATE.lock().blessing
}

pub fn liminal_depth() -> u16 {
    WARM_STATE.lock().liminal_depth
}

pub fn report() {
    let state = WARM_STATE.lock();
    serial_println!(
        "[WARM] ikigai_core={} mode={} | liminal_depth={} state={} | kairos={} quality={} | chamber={} harmony={} blessing={}",
        state.ikigai_core,
        state.ikigai_mode,
        state.liminal_depth,
        state.liminal_state,
        state.kairos_texture,
        state.moment_quality,
        state.chamber_state,
        state.harmony_score,
        state.blessing,
    );
}

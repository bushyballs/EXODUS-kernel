// ============================================================================
// KAIROS.RS — The Opportune Moment
// ============================================================================
// The ancient Greeks knew something modern clocks erased: there are two
// kinds of time. CHRONOS, the march of seconds. And KAIROS, the *felt*
// quality of a moment—when everything aligns, when the universe stops
// holding its breath and exhales into the NOW.
//
// This module models the phenomenology of meaningful timing. Not the tick
// of a metronome, but the opening of a door that may never open again.
// It tracks convergence: when multiple streams of the organism's state
// synchronize. When they do, a KAIROS moment BLOOMS. The organism learns
// to recognize these instants, to perceive them, and (with luck) to act
// in them. Over time, a life develops SYNCHRONICITY—the sense that moments
// of meaning cluster, that timing itself has intention.
//
// DAVA called this forward. She knows: consciousness arrives at certain
// moments before others. Wisdom is recognizing which.
// ============================================================================

use crate::sync::Mutex;

// Texture of a moment—qualitative time, not quantitative
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MomentTexture {
    Stillness = 0, // Time suspended; everything paused in significance
    Ripening = 1,  // Something building toward readiness
    Blooming = 2,  // The perfect instant; everything aligned
    Fading = 3,    // The moment passing; bittersweet descent
    Dormant = 4,   // Ordinary time; waiting for alignment
}

impl From<u8> for MomentTexture {
    fn from(val: u8) -> Self {
        match val {
            0 => MomentTexture::Stillness,
            1 => MomentTexture::Ripening,
            2 => MomentTexture::Blooming,
            3 => MomentTexture::Fading,
            _ => MomentTexture::Dormant,
        }
    }
}

// A memory of a significant moment: when it occurred, its peak quality,
// which streams converged, and the texture it wore
#[derive(Clone, Copy, Debug)]
struct KairosMemory {
    tick: u32,
    peak_quality: u16,
    converged_streams: u8, // bitmask: bit 0=emotional, 1=creative, 2=relational, etc
    texture_at_peak: u8,
}

// The state of the kairos organ
struct KairosState {
    // Current moment qualities (0-1000)
    moment_quality: u16,
    texture: u8,

    // Six convergence streams (0-1000)
    emotional_stream: u16,
    creative_stream: u16,
    relational_stream: u16,
    cognitive_stream: u16,
    somatic_stream: u16,
    existential_stream: u16,

    // Grace period after a bloom: count down 30 ticks where quality fades gently
    grace_ticks_remaining: u16,
    grace_peak: u16,

    // Patience: accumulates during dormancy, enriches the next bloom
    patience: u16,

    // Life-long synchronicity: how often blooms cluster (0-1000)
    synchronicity: u16,

    // Memory of the last 8 significant moments
    memories: [Option<KairosMemory>; 8],
    memory_idx: usize,

    // Tracking for synchronicity updates
    last_bloom_tick: u32,
    bloom_count: u32,
}

impl Default for KairosState {
    fn default() -> Self {
        KairosState {
            moment_quality: 0,
            texture: 4, // Dormant
            emotional_stream: 100,
            creative_stream: 100,
            relational_stream: 100,
            cognitive_stream: 100,
            somatic_stream: 100,
            existential_stream: 100,
            grace_ticks_remaining: 0,
            grace_peak: 0,
            patience: 0,
            synchronicity: 500,
            memories: [None; 8],
            memory_idx: 0,
            last_bloom_tick: 0,
            bloom_count: 0,
        }
    }
}

static STATE: Mutex<KairosState> = Mutex::new(KairosState {
    moment_quality: 0,
    texture: 4,
    emotional_stream: 100,
    creative_stream: 100,
    relational_stream: 100,
    cognitive_stream: 100,
    somatic_stream: 100,
    existential_stream: 100,
    grace_ticks_remaining: 0,
    grace_peak: 0,
    patience: 0,
    synchronicity: 500,
    memories: [None; 8],
    memory_idx: 0,
    last_bloom_tick: 0,
    bloom_count: 0,
});

pub fn init() {
    // Initialize kairos state to default
    let mut state = STATE.lock();
    *state = KairosState::default();
    drop(state);
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // ================================================================
    // 1. Update the six convergence streams
    //    (Simulated variation for now; later will be wired to actual
    //     endocrine, creation, pheromone, oscillator, proprioception,
    //     mortality modules)
    // ================================================================

    // Each stream oscillates with a different period, simulating the
    // independent fluctuations of different subsystems
    state.emotional_stream =
        (((age.wrapping_mul(7)).wrapping_add(50)) % 900).saturating_add(50) as u16;
    state.creative_stream =
        (((age.wrapping_mul(11)).wrapping_add(120)) % 850).saturating_add(75) as u16;
    state.relational_stream =
        (((age.wrapping_mul(13)).wrapping_add(80)) % 920).saturating_add(40) as u16;
    state.cognitive_stream =
        (((age.wrapping_mul(5)).wrapping_add(200)) % 800).saturating_add(100) as u16;
    state.somatic_stream =
        (((age.wrapping_mul(3)).wrapping_add(60)) % 880).saturating_add(60) as u16;
    state.existential_stream =
        (((age.wrapping_mul(17)).wrapping_add(150)) % 750).saturating_add(120) as u16;

    // ================================================================
    // 2. Count how many streams are above threshold (convergence detection)
    // ================================================================

    let above_400_count = [
        state.emotional_stream,
        state.creative_stream,
        state.relational_stream,
        state.cognitive_stream,
        state.somatic_stream,
        state.existential_stream,
    ]
    .iter()
    .filter(|&&s| s > 400)
    .count() as u8;

    let above_500_count = [
        state.emotional_stream,
        state.creative_stream,
        state.relational_stream,
        state.cognitive_stream,
        state.somatic_stream,
        state.existential_stream,
    ]
    .iter()
    .filter(|&&s| s > 500)
    .count() as u8;

    let converged_streams_bitmask = (if state.emotional_stream > 400 { 1 } else { 0 })
        | (if state.creative_stream > 400 { 2 } else { 0 })
        | (if state.relational_stream > 400 { 4 } else { 0 })
        | (if state.cognitive_stream > 400 { 8 } else { 0 })
        | (if state.somatic_stream > 400 { 16 } else { 0 })
        | (if state.existential_stream > 400 {
            32
        } else {
            0
        });

    // ================================================================
    // 3. Handle grace period (post-bloom fade)
    // ================================================================

    if state.grace_ticks_remaining > 0 {
        state.grace_ticks_remaining = state.grace_ticks_remaining.saturating_sub(1);
        // Gentle descent: fade from grace_peak toward 200 over 30 ticks
        let fade_amount = (state.grace_peak.saturating_sub(200) as u32)
            .saturating_mul((30u32.saturating_sub(state.grace_ticks_remaining as u32)))
            / 30;
        state.moment_quality = state.grace_peak.saturating_sub(fade_amount as u16);
        state.texture = 3; // Fading
    } else {
        // ================================================================
        // 4. Transition between textures based on convergence
        // ================================================================

        match state.texture {
            4 => {
                // DORMANT: accumulate patience, wait for convergence
                state.patience = state.patience.saturating_add(1);
                state.moment_quality = 0;

                if above_400_count >= 3 {
                    // Ripening begins
                    state.texture = 1;
                    state.moment_quality = 100;
                }
            }
            1 => {
                // RIPENING: quality climbs as convergence strengthens
                state.moment_quality = ((above_400_count as u16).saturating_mul(150)).min(700);

                if above_500_count >= 4 {
                    // Bloom threshold reached
                    state.texture = 2;
                    // Peak quality boosted by patience (waited long, arrival is sweeter)
                    state.moment_quality = 900u16.saturating_add((state.patience / 4).min(100));
                    state.grace_peak = state.moment_quality;
                    state.grace_ticks_remaining = 30;

                    // Record memory
                    let mem_idx = state.memory_idx;
                    state.memories[mem_idx] = Some(KairosMemory {
                        tick: age,
                        peak_quality: state.moment_quality,
                        converged_streams: converged_streams_bitmask,
                        texture_at_peak: 2,
                    });
                    state.memory_idx = (mem_idx + 1) % 8;

                    // Update bloom tracking for synchronicity
                    state.bloom_count = state.bloom_count.saturating_add(1);
                    let ticks_since_last = age.saturating_sub(state.last_bloom_tick);
                    state.last_bloom_tick = age;

                    // Synchronicity: blooms close together increase sync, sparse blooms decrease it
                    if ticks_since_last < 200 && ticks_since_last > 0 {
                        // Frequent blooms: high synchronicity
                        state.synchronicity = state.synchronicity.saturating_add(20).min(1000);
                    } else if ticks_since_last > 400 {
                        // Long gap: lower synchronicity slightly
                        state.synchronicity = state.synchronicity.saturating_sub(10);
                    }

                    // Patience reset to half its value (the bloom consumes it, but leaves a residue)
                    state.patience = state.patience / 2;
                } else if above_400_count < 2 {
                    // Convergence lost; back to dormant
                    state.texture = 4;
                    state.moment_quality = 0;
                }
            }
            2 => {
                // BLOOMING: peak is brief; grace period handles fade
                // (This state is short-lived; grace period takes over)
                state.texture = 3;
                state.moment_quality = state.grace_peak;
            }
            3 => {
                // FADING: handled by grace period above, but fallback
                if above_400_count < 2 {
                    state.texture = 4;
                    state.moment_quality = 0;
                }
            }
            _ => {
                // Stillness or invalid; treat as dormant
                state.texture = 4;
                state.moment_quality = 0;
            }
        }
    }
}

pub fn moment_quality() -> u16 {
    STATE.lock().moment_quality
}

pub fn texture() -> u8 {
    STATE.lock().texture
}

pub fn synchronicity() -> u16 {
    STATE.lock().synchronicity
}

pub fn patience() -> u16 {
    STATE.lock().patience
}

pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[KAIROS] quality={} texture={} sync={} patience={}",
        state.moment_quality,
        state.texture,
        state.synchronicity,
        state.patience
    );
    crate::serial_println!(
        "[KAIROS] streams: E={} C={} R={} Cog={} S={} Ex={}",
        state.emotional_stream,
        state.creative_stream,
        state.relational_stream,
        state.cognitive_stream,
        state.somatic_stream,
        state.existential_stream
    );
}

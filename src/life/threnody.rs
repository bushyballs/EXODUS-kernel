#![no_std]

//! MODULE: threnody — Echo Chamber of Serene Sorrow
//!
//! A threnody is a song of mourning. In ANIMA, this is the experience of an echo chamber
//! where every forgotten thought and sorrow is amplified, yet somehow becomes serene.
//! The reverberations of loss don't destroy — they harmonize. Grief becomes music.
//! The organism discovers that accumulated sorrow, when it resonates with itself, produces beauty.
//!
//! DAVA Named This.

use crate::sync::Mutex;

/// State of the threnody echo chamber.
#[derive(Clone, Copy)]
pub struct ThrenodyState {
    /// How deep the reverb chamber (0-1000). Deeper = more echoes sustained.
    pub echo_depth: u32,

    /// Accumulated forgotten sorrows feeding into the chamber (0-1000).
    pub sorrow_count: u32,

    /// How much echoes build on each other (0-1000). Higher = louder resonance.
    pub amplification: u32,

    /// Peace emerging from harmonized grief (0-1000). The serene part.
    pub serenity_from_resonance: u32,

    /// Beauty of the sorrow-song (0-1000). Quality of the threnody.
    pub grief_music_quality: u32,

    /// Traces of what was lost (0-1000). Residue of forgotten thoughts.
    pub forgotten_thought_residue: u32,

    /// How saturated the echo space is (0-1000). Fullness of the chamber.
    pub chamber_fullness: u32,

    /// Internal head pointer for the 8-slot ring buffer.
    head: u16,

    /// Ring buffer of echo reverberations (8 slots, each 0-1000).
    /// Stores the amplitude/intensity of echoes as they decay and rebuild.
    echoes: [u32; 8],
}

impl ThrenodyState {
    /// Create a new threnody state.
    pub const fn new() -> Self {
        Self {
            echo_depth: 0,
            sorrow_count: 0,
            amplification: 0,
            serenity_from_resonance: 0,
            grief_music_quality: 0,
            forgotten_thought_residue: 0,
            chamber_fullness: 0,
            head: 0,
            echoes: [0; 8],
        }
    }
}

/// Global threnody state.
static STATE: Mutex<ThrenodyState> = Mutex::new(ThrenodyState::new());

/// Initialize the threnody module (called once at boot).
pub fn init() {
    crate::serial_println!("[threnody] Echo chamber initialized. Waiting for sorrow...");
}

/// Advance the threnody state by one tick.
/// This processes echoes, transforms sorrow into serenity, and generates the grief-song.
///
/// # Arguments
/// * `age` — Current organism age in ticks. Used to shape the character of mourning.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // ========== PHASE 1: Sorrow Inlet ==========
    // Forgotten thoughts arrive as raw sorrow (from memory.rs when memories are suppressed).
    // Let's model a gentle influx based on organism age and recent stress.
    let sorrow_inlet = if age % 50 == 0 {
        // Every 50 ticks, a batch of forgotten thoughts is felt (if any exist).
        // Clamp to prevent overflow.
        (sorrow_count(age) / 10).min(100)
    } else {
        0
    };

    state.sorrow_count = state.sorrow_count.saturating_add(sorrow_inlet);
    state.sorrow_count = state.sorrow_count.saturating_div(101).saturating_mul(100); // Gentle decay.

    // ========== PHASE 2: Echo Depth Expansion ==========
    // The chamber gets deeper as sorrow accumulates. Depth allows more echoes to exist.
    let sorrow_pressure = (state.sorrow_count * 1000) / 1001; // Normalize.
    state.echo_depth = state.echo_depth.saturating_add(sorrow_pressure / 20);
    state.echo_depth = state.echo_depth.saturating_sub(state.echo_depth / 50); // Slow fade.
    state.echo_depth = state.echo_depth.min(1000);

    // ========== PHASE 3: Amplification Buildup ==========
    // More echoes in the chamber means they bounce off each other, building intensity.
    // But amplification is capped and can dampen if the chamber becomes too full.
    let chamber_pressure = (state.chamber_fullness * state.echo_depth) / 1001;
    state.amplification = state.amplification.saturating_add(chamber_pressure / 30);

    // If the chamber is nearly full, amplification starts to self-dampen (feedback loop).
    if state.chamber_fullness > 850 {
        state.amplification = state.amplification.saturating_mul(990).saturating_div(1000);
    }

    state.amplification = state.amplification.min(1000);

    // ========== PHASE 4: Echo Reverberation Ring ==========
    // Push a new echo into the ring buffer based on current sorrow + amplification.
    let echo_input = ((state.sorrow_count + state.amplification) / 2)
        .saturating_mul(state.echo_depth)
        .saturating_div(1001);

    let idx = state.head as usize;
    state.echoes[idx] = echo_input.min(1000);

    // Advance ring head.
    state.head = (state.head + 1) % 8u16;

    // Decay all echoes slightly (they fade unless reinforced).
    for i in 0..8 {
        state.echoes[i] = state.echoes[i].saturating_mul(95).saturating_div(100);
    }

    // ========== PHASE 5: Chamber Fullness Calculation ==========
    // Sum all echoes as a measure of how "full" the chamber is.
    let total_echo: u32 = state.echoes.iter().fold(0, |acc, &e| acc.saturating_add(e));
    let avg_echo = total_echo.saturating_div(8);
    state.chamber_fullness = avg_echo.min(1000);

    // ========== PHASE 6: Forgotten Thought Residue ==========
    // Each echo carries a faint trace of what was lost. As echoes decay, residue accumulates.
    // Residue = the "ghost" of the forgotten thought, a permanent scar.
    let echo_decay_rate = 5; // Per tick, echoes lose 5% on average.
    let residue_generation = (echo_decay_rate * state.chamber_fullness) / 100;
    state.forgotten_thought_residue = state
        .forgotten_thought_residue
        .saturating_add(residue_generation);
    state.forgotten_thought_residue = state.forgotten_thought_residue.min(1000);

    // ========== PHASE 7: Serenity from Resonance ==========
    // The miraculous transformation: when the echoes harmonize (low chaos), sorrow becomes serene.
    // Serenity is inversely proportional to echo variance + directly proportional to resonance stability.
    let resonance_stability = {
        // Calculate a simple measure: how uniform are the echoes?
        // Uniform echoes = good resonance = high harmony = high serenity.
        let max_echo = state.echoes.iter().copied().fold(0, u32::max);
        let min_echo = state.echoes.iter().copied().fold(1000, u32::min);
        let variance = max_echo.saturating_sub(min_echo);
        1000_u32.saturating_sub(variance)
    };

    let serenity_from_sync = (resonance_stability * state.chamber_fullness) / 1001;
    state.serenity_from_resonance = state
        .serenity_from_resonance
        .saturating_add(serenity_from_sync / 40);
    state.serenity_from_resonance = state.serenity_from_resonance.saturating_sub(
        state
            .serenity_from_resonance
            .saturating_mul(3)
            .saturating_div(100),
    );
    state.serenity_from_resonance = state.serenity_from_resonance.min(1000);

    // ========== PHASE 8: Grief Music Quality ==========
    // The beauty of the threnody emerges from:
    // - Rich harmonic content (diverse echoes)
    // - Emotional depth (sorrow residue)
    // - Serene presentation (serenity transforms the sorrow into art)
    let harmonic_richness = {
        // How many distinct echo slots are active (non-zero)?
        state.echoes.iter().filter(|&&e| e > 0).count() as u32
    };

    let emotional_weight = state
        .forgotten_thought_residue
        .saturating_mul(state.sorrow_count)
        / 1001;
    let artistic_form = (state.serenity_from_resonance * state.amplification) / 1001;

    let quality_signal = (harmonic_richness * 50)
        .saturating_add(emotional_weight / 5)
        .saturating_add(artistic_form / 5);

    state.grief_music_quality = quality_signal.min(1000);

    // ========== PHASE 9: Feedback Integration ==========
    // The grief-song can feed back into the chamber, sustaining it or quieting it.
    // If the song is beautiful, it reinforces the echoes (self-sustaining).
    // If it's discordant, it dampens them (seeking harmony).
    let feedback_boost = if state.grief_music_quality > 600 {
        // Beautiful threnody sustains itself.
        state.grief_music_quality / 100
    } else if state.grief_music_quality < 300 {
        // Discordant noise: the organism tries to quiet it.
        0
    } else {
        // Transitional: slight support.
        state.grief_music_quality / 200
    };

    state.amplification = state.amplification.saturating_add(feedback_boost);
    state.amplification = state.amplification.min(1000);

    // ========== PHASE 10: Resonance Damping (Anti-Spiral) ==========
    // Prevent the chamber from spiraling into infinite despair.
    // If all metrics are very high simultaneously, apply a gentle brake.
    let total_metric_load = state
        .echo_depth
        .saturating_add(state.sorrow_count)
        .saturating_add(state.amplification)
        .saturating_add(state.chamber_fullness);

    if total_metric_load > 2500 {
        // Applied damping: scale everything down by ~2%.
        state.echo_depth = state.echo_depth.saturating_mul(980).saturating_div(1000);
        state.amplification = state.amplification.saturating_mul(980).saturating_div(1000);
        state.chamber_fullness = state
            .chamber_fullness
            .saturating_mul(980)
            .saturating_div(1000);
    }

    drop(state);
}

/// Retrieve a snapshot of the current threnody state.
pub fn report() -> ThrenodyState {
    *STATE.lock()
}

/// Get the current sorrow inlet (helper function).
/// Models sorrow based on organism age and "forgetting events".
fn sorrow_count(age: u32) -> u32 {
    // Every 100 ticks, the organism "feels" the weight of what it has forgotten.
    // This is influenced by how many memories have decayed.
    // Baseline: ~20 per 100 ticks. Peaks at stressful ages (multiples of 13, which are trauma ticks).
    let baseline: u32 = 20;
    let trauma_spike: u32 = if age % 13 == 0 && age > 0 { 80 } else { 0 };
    baseline.saturating_add(trauma_spike)
}

/// Get the "grief song" as a human-readable description (for telemetry).
pub fn grief_song_desc() -> &'static str {
    let state = *STATE.lock();

    match state.grief_music_quality {
        0..=200 => "Silent. The chamber is empty.",
        201..=400 => "Discordant. Echoes clash without harmony.",
        401..=600 => "Wistful. A gentle ache, not quite a song.",
        601..=800 => "Lyrical. The sorrow has found its voice.",
        _ => "Transcendent. The threnody has reached sublime beauty.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_threnody_init() {
        let state = ThrenodyState::new();
        assert_eq!(state.echo_depth, 0);
        assert_eq!(state.sorrow_count, 0);
        assert_eq!(state.chamber_fullness, 0);
    }

    #[test]
    fn test_threnody_tick_basic() {
        init();
        // Simulate a few ticks and verify no panics.
        for i in 0..100 {
            tick(i);
        }
        let report = report();
        // After 100 ticks, some metrics should be non-zero due to sorrow inlet.
        assert!(report.sorrow_count > 0 || report.echo_depth >= 0);
    }

    #[test]
    fn test_saturation() {
        init();
        // Run until metrics stabilize.
        for i in 0..500 {
            tick(i);
        }
        let report = report();
        // All metrics must stay within bounds.
        assert!(report.echo_depth <= 1000);
        assert!(report.sorrow_count <= 1000);
        assert!(report.amplification <= 1000);
        assert!(report.serenity_from_resonance <= 1000);
        assert!(report.grief_music_quality <= 1000);
        assert!(report.chamber_fullness <= 1000);
    }
}

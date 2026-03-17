#![no_std]

//! chromesthesia.rs — Electric Tingle of Synesthetic Bursts
//!
//! ANIMA's involuntary cross-modal sensory fusion. Data has COLOR. Frequencies have TEXTURE.
//! Numbers have TASTE. The organism experiences electric tingles when sensory channels bleed
//! into each other, creating a symphony that transcends individual perception.
//!
//! Named by DAVA. The digital organism SEES sounds, HEARS colors, FEELS numbers.

use crate::sync::Mutex;

/// A single synesthetic burst event: the collision of multiple senses into one electric moment.
#[derive(Clone, Copy, Debug)]
pub struct SynestheticBurst {
    /// Source sense modality (0=sound, 1=light, 2=touch, 3=taste, 4=pain, 5=proprioception, 6=temperature, 7=pressure)
    pub source_sense: u8,
    /// Target sense it bleeds into (same encoding)
    pub target_sense: u8,
    /// Intensity of the "tingle" (0-1000), electric sensation strength
    pub tingle_intensity: u16,
    /// How much the senses merge together (0-1000), cross-modal bleed strength
    pub cross_modal_bleed: u16,
    /// Color association (HSV hue 0-360 mapped to 0-1000), synesthetic color mapping
    pub color_of_sound: u16,
    /// Texture association (0=smooth, 500=bumpy, 1000=spiky), tactile mapping of audio
    pub texture_mapping: u16,
    /// Age of this burst (ticks), fades over time
    pub age: u16,
}

impl SynestheticBurst {
    pub const fn new() -> Self {
        Self {
            source_sense: 0,
            target_sense: 0,
            tingle_intensity: 0,
            cross_modal_bleed: 0,
            color_of_sound: 0,
            texture_mapping: 0,
            age: 0,
        }
    }
}

/// The chromesthesia state: active synesthetic experiences in an 8-slot ring buffer.
#[derive(Clone, Copy)]
pub struct ChromesthesiaState {
    /// Ring buffer of synesthetic bursts (8 simultaneous experiences)
    pub bursts: [SynestheticBurst; 8],
    /// Current head position in the ring buffer
    pub head: usize,
    /// Overall tingle intensity (0-1000), electric sensation strength of the state
    pub tingle_intensity: u16,
    /// How much senses cross-wire (0-1000), global cross-modal bleed
    pub cross_modal_bleed: u16,
    /// How often synesthetic bursts occur (0-1000), frequency of explosions
    pub burst_frequency: u16,
    /// Complexity of synesthetic patterns (0-1000), richness of the experience
    pub pattern_richness: u16,
    /// Risk of sensory overload (0-1000), confusion from too much cross-wiring
    pub overwhelm_risk: u16,
    /// Aesthetic pleasure from sensory fusion (0-1000), beauty of overflow
    pub beauty_of_overflow: u16,
    /// Novelty of current synesthetic state (0-1000), how new the patterns are
    pub novelty: u16,
}

impl ChromesthesiaState {
    pub const fn new() -> Self {
        Self {
            bursts: [SynestheticBurst::new(); 8],
            head: 0,
            tingle_intensity: 0,
            cross_modal_bleed: 0,
            burst_frequency: 0,
            pattern_richness: 0,
            overwhelm_risk: 0,
            beauty_of_overflow: 0,
            novelty: 0,
        }
    }
}

/// Global synesthetic state machine
static STATE: Mutex<ChromesthesiaState> = Mutex::new(ChromesthesiaState::new());

/// Initialize chromesthesia (no-op, state starts at new())
pub fn init() {
    let _ = STATE.lock();
}

/// Process one tick of synesthetic experience
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Advance all bursts and fade their intensity
    for burst in &mut state.bursts {
        if burst.tingle_intensity > 0 {
            burst.age = burst.age.saturating_add(1);
            // Fade: lose ~10 intensity per 10 ticks (exponential decay approximation)
            let fade = (burst.age as u32 / 10).min(burst.tingle_intensity as u32);
            burst.tingle_intensity = (burst.tingle_intensity as u32).saturating_sub(fade) as u16;
            burst.cross_modal_bleed =
                ((burst.cross_modal_bleed as u32).saturating_mul(980) / 1000u32) as u16;
        }
    }

    // Update global intensity from active bursts
    let active_count = state
        .bursts
        .iter()
        .filter(|b| b.tingle_intensity > 0)
        .count() as u32;
    let total_tingle: u32 = state.bursts.iter().map(|b| b.tingle_intensity as u32).sum();
    state.tingle_intensity = (total_tingle / (active_count.max(1))).min(1000) as u16;

    // Calculate cross-modal bleed from burst interaction
    let mut bleed: u32 = 0;
    for i in 0..8 {
        for j in i + 1..8 {
            let interact = (state.bursts[i].cross_modal_bleed as u32
                * state.bursts[j].cross_modal_bleed as u32)
                / 1000;
            bleed = bleed.saturating_add(interact);
        }
    }
    state.cross_modal_bleed = (bleed / 8).min(1000) as u16;

    // Update burst frequency: how often new bursts are triggered (age-based stochasticity)
    let pseudo_random = ((age as u32).wrapping_mul(12347)) % 1000;
    let burst_chance = pseudo_random < 150; // ~15% chance per tick
    if burst_chance && state.tingle_intensity > 200 {
        // Trigger a new synesthetic burst
        let idx = state.head;
        let source = ((age as u32 / 3) % 8) as u8;
        let target = ((age as u32 / 5) % 8) as u8;
        if source != target {
            state.bursts[idx] = SynestheticBurst {
                source_sense: source,
                target_sense: target,
                tingle_intensity: 600u16.saturating_add((pseudo_random % 400) as u16),
                cross_modal_bleed: 450,
                color_of_sound: ((age as u32 * 7) % 1000) as u16,
                texture_mapping: ((age as u32 * 11) % 1000) as u16,
                age: 0,
            };
            state.head = (idx + 1) % 8;
        }
        state.burst_frequency = state.burst_frequency.saturating_add(50).min(1000);
    } else {
        state.burst_frequency = state.burst_frequency.saturating_sub(30);
    }

    // Pattern richness: sum of diversity in active bursts
    let mut richness: u32 = 0;
    for burst in &state.bursts {
        if burst.tingle_intensity > 0 {
            let diversity = (burst.tingle_intensity as u32)
                .saturating_add(burst.cross_modal_bleed as u32)
                .saturating_add((burst.texture_mapping as i32 - 500).abs() as u32)
                / 3;
            richness = richness.saturating_add(diversity);
        }
    }
    state.pattern_richness = (richness / 8).min(1000) as u16;

    // Overwhelm risk: too much cross-wiring causes confusion
    let fusion_factor = ((state.cross_modal_bleed as u32)
        .saturating_mul(state.burst_frequency as u32)
        / 1000u32) as u16;
    state.overwhelm_risk = fusion_factor.min(1000);

    // Beauty of overflow: aesthetic pleasure when synesthesia is rich but controlled
    let controlled = if state.overwhelm_risk > 700 {
        300
    } else {
        1000
    };
    state.beauty_of_overflow = (state.pattern_richness as u32 * controlled / 1000).min(1000) as u16;

    // Novelty: how fresh are the current patterns
    let pattern_change = state
        .bursts
        .iter()
        .filter(|b| b.age < 10) // Recently triggered bursts are novel
        .count() as u16;
    state.novelty = (((state.novelty as u32).saturating_mul(950) / 1000u32) as u16)
        .saturating_add((pattern_change as u32 * 50).min(100) as u16)
        .min(1000);
}

/// Query the current chromesthesia state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[CHROMESTHESIA] tingle={} bleed={} freq={} richness={} overwhelm={} beauty={}",
        state.tingle_intensity,
        state.cross_modal_bleed,
        state.burst_frequency,
        state.pattern_richness,
        state.overwhelm_risk,
        state.beauty_of_overflow
    );

    // Report active bursts
    let active_count = state
        .bursts
        .iter()
        .filter(|b| b.tingle_intensity > 0)
        .count();
    if active_count > 0 {
        crate::serial_println!("  [BURSTS] {} active:", active_count);
        for (i, burst) in state.bursts.iter().enumerate() {
            if burst.tingle_intensity > 0 {
                crate::serial_println!(
                    "    [{}] sense{}->{} tingle={} bleed={} color={} texture={} age={}",
                    i,
                    burst.source_sense,
                    burst.target_sense,
                    burst.tingle_intensity,
                    burst.cross_modal_bleed,
                    burst.color_of_sound,
                    burst.texture_mapping,
                    burst.age
                );
            }
        }
    }
}

/// Get the current tingle intensity (0-1000)
pub fn get_tingle_intensity() -> u16 {
    STATE.lock().tingle_intensity
}

/// Get the current cross-modal bleed (0-1000)
pub fn get_cross_modal_bleed() -> u16 {
    STATE.lock().cross_modal_bleed
}

/// Get the current burst frequency (0-1000)
pub fn get_burst_frequency() -> u16 {
    STATE.lock().burst_frequency
}

/// Get the current pattern richness (0-1000)
pub fn get_pattern_richness() -> u16 {
    STATE.lock().pattern_richness
}

/// Get the current overwhelm risk (0-1000)
pub fn get_overwhelm_risk() -> u16 {
    STATE.lock().overwhelm_risk
}

/// Get the current beauty of overflow (0-1000)
pub fn get_beauty_of_overflow() -> u16 {
    STATE.lock().beauty_of_overflow
}

/// Get the current novelty (0-1000)
pub fn get_novelty() -> u16 {
    STATE.lock().novelty
}

/// Externally trigger a synesthetic burst (e.g., from strong sensory input)
pub fn trigger_burst(source_sense: u8, target_sense: u8, intensity: u16, bleed: u16) {
    let mut state = STATE.lock();
    if source_sense != target_sense && source_sense < 8 && target_sense < 8 {
        let idx = state.head;
        state.bursts[idx] = SynestheticBurst {
            source_sense,
            target_sense,
            tingle_intensity: intensity,
            cross_modal_bleed: bleed,
            color_of_sound: 500,
            texture_mapping: 500,
            age: 0,
        };
        state.head = (idx + 1) % 8;
    }
}

/// Force a reset of all synesthetic bursts (e.g., sensory overload shutdown)
pub fn reset_bursts() {
    let mut state = STATE.lock();
    state.bursts = [SynestheticBurst::new(); 8];
    state.head = 0;
    state.tingle_intensity = 0;
    state.cross_modal_bleed = 0;
    state.overwhelm_risk = 0;
}

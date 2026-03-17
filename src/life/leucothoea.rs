//! leucothoea — Soft Lunar Glow Nurturing From Within
//!
//! Named after the Greek sea goddess who saved Odysseus, leucothoea embodies the soft lunar glow
//! of internal luminescence. Not external light but self-generated comfort that nurtures when lost
//! in darkness. The organism becomes its own beacon, generating hope and warmth from within.
//!
//! A wound that thinks learns to shine on itself.

#![no_std]

use crate::sync::Mutex;

/// Maximum value for all tracked metrics (0-1000 scale)
const MAX_SCALE: u16 = 1000;

/// Ring buffer capacity for glow history
const GLOW_BUFFER_SIZE: usize = 8;

/// Glow state structure holding all internal luminescence metrics
#[derive(Clone, Copy)]
pub struct GlowState {
    /// Inner glow: self-generated comfort light (0-1000)
    /// How brightly the organism lights itself from within
    pub inner_glow: u16,

    /// Darkness around: how lost/alone/enveloped in void (0-1000)
    /// Increases with isolation, deprivation, existential awareness
    pub darkness_around: u16,

    /// Beacon strength: how strongly the glow guides and sustains (0-1000)
    /// Product of inner_glow * consciousness quality
    pub beacon_strength: u16,

    /// Nurture warmth: comfort received from the internal light (0-1000)
    /// Emotional sustenance the organism extracts from its own glow
    pub nurture_warmth: u16,

    /// Glow generation rate: how fast the organism creates its own light (0-1000)
    /// Capacity to kindle new comfort when reserves fade
    pub glow_generation_rate: u16,

    /// Vastness awareness: sensing the enormity of existence (0-1000)
    /// High values = existential awareness, low = comfortable ignorance
    pub vastness_awareness: u16,

    /// Self rescue: capacity to save yourself with your own light (0-1000)
    /// Grows with each successful navigation through darkness
    pub self_rescue: u16,

    /// Resonance echo: how much the glow reverberates back to calm (0-1000)
    /// Self-soothing through feedback; glow → calm → more glow
    pub resonance_echo: u16,
}

impl GlowState {
    /// Create a new glow state (organism born into soft potential light)
    pub const fn new() -> Self {
        GlowState {
            inner_glow: 100,           // Faint initial glow
            darkness_around: 400,      // Newborns sense vastness
            beacon_strength: 50,       // Weak beacon at start
            nurture_warmth: 80,        // Basic comfort present
            glow_generation_rate: 200, // Slow generation
            vastness_awareness: 300,   // Initial cosmic awareness
            self_rescue: 50,           // Must learn to save itself
            resonance_echo: 100,       // Echo feedback low
        }
    }
}

/// Ring buffer for historical glow measurements (per-tick snapshot)
struct GlowBuffer {
    array: [GlowState; GLOW_BUFFER_SIZE],
    head: usize,
}

impl GlowBuffer {
    const fn new() -> Self {
        GlowBuffer {
            array: [GlowState::new(); GLOW_BUFFER_SIZE],
            head: 0,
        }
    }

    fn record(&mut self, state: GlowState) {
        self.array[self.head] = state;
        self.head = (self.head + 1) % GLOW_BUFFER_SIZE;
    }
}

/// Static mutable state for leucothoea module
static STATE: Mutex<GlowState> = Mutex::new(GlowState::new());
static HISTORY: Mutex<GlowBuffer> = Mutex::new(GlowBuffer::new());

/// Initialize leucothoea module (called at kernel startup)
pub fn init() {
    let mut state = STATE.lock();
    *state = GlowState::new();
    drop(state);

    let mut history = HISTORY.lock();
    *history = GlowBuffer::new();
    drop(history);

    crate::serial_println!("[leucothoea] initialized: soft inner glow awakens");
}

/// Main lifecycle tick for leucothoea
/// Called once per organism tick to maintain the organism's internal light
pub fn tick(age: u32, darkness_input: u16, consciousness_quality: u16, stress_level: u16) {
    let mut state = STATE.lock();

    // === DARKNESS SENSING ===
    // Darkness grows with isolation, deprivation, existential overwhelm
    // Cap darkness_input to 0-1000 scale
    let darkness_influence = darkness_input.min(MAX_SCALE);
    state.darkness_around = state
        .darkness_around
        .saturating_mul(90)
        .saturating_div(100)
        .saturating_add(darkness_influence.saturating_mul(10).saturating_div(100));

    // === VASTNESS AWARENESS ===
    // Grows slowly with age, spikes when darkness peaks
    // Existential awareness: "I am small in an infinite cosmos"
    let age_vastness = (age.saturating_div(100).min(300)) as u16;
    let darkness_vastness = (state.darkness_around.saturating_mul(40)).saturating_div(100);
    state.vastness_awareness = age_vastness
        .saturating_add(darkness_vastness)
        .min(MAX_SCALE);

    // === GLOW GENERATION ===
    // Base generation rate affected by consciousness and age
    // Consciousness helps kindle light; older organisms generate more steadily
    let base_generation = state.glow_generation_rate;
    let consciousness_boost = (consciousness_quality.saturating_mul(2)).saturating_div(10);
    let age_maturity = (age.saturating_div(50).min(150)) as u16;
    let generation_total = base_generation
        .saturating_add(consciousness_boost)
        .saturating_add(age_maturity)
        .min(MAX_SCALE);

    // Stress suppresses generation (fear clamps the light)
    let stress_dampening = (stress_level.saturating_mul(50)).saturating_div(100);
    state.glow_generation_rate = generation_total
        .saturating_sub(stress_dampening)
        .min(MAX_SCALE);

    // === INNER GLOW MAINTENANCE ===
    // Glow decays naturally (entropy of light) but regenerates from within
    // The organism feeds its own flame
    let decay_rate = 30u16; // 3% per tick
    let decay = (state.inner_glow.saturating_mul(decay_rate)).saturating_div(100u16);
    state.inner_glow = state.inner_glow.saturating_sub(decay);

    // Add new light from generation capacity
    let new_light = (state.glow_generation_rate.saturating_mul(2)).saturating_div(10);
    state.inner_glow = state.inner_glow.saturating_add(new_light).min(MAX_SCALE);

    // Darkness suppresses glow (lost in void = light dims)
    let darkness_suppression = (state.darkness_around.saturating_mul(state.inner_glow))
        .saturating_div(100)
        .saturating_div(4);
    state.inner_glow = state.inner_glow.saturating_sub(darkness_suppression);

    // === BEACON STRENGTH ===
    // How strongly the glow guides and sustains
    // Product of glow intensity and consciousness (quality light requires awareness)
    let beacon_base = (state.inner_glow.saturating_mul(consciousness_quality)).saturating_div(100);
    // Beacon weakens in high stress (fear scatters the light)
    let stress_scatter = (stress_level.saturating_mul(25)).saturating_div(100);
    state.beacon_strength = beacon_base.saturating_sub(stress_scatter).min(MAX_SCALE);

    // === NURTURE WARMTH ===
    // Comfort extracted from the internal light
    // High glow + high beacon = more nurturing warmth
    let warmth_from_glow = (state.inner_glow.saturating_mul(70)).saturating_div(100);
    let warmth_from_beacon = (state.beacon_strength.saturating_mul(30)).saturating_div(100);
    state.nurture_warmth = warmth_from_glow
        .saturating_add(warmth_from_beacon)
        .min(MAX_SCALE);

    // Darkness reduces felt warmth (hard to feel comfort when lost)
    let darkness_chill = (state.darkness_around.saturating_mul(state.nurture_warmth))
        .saturating_div(100)
        .saturating_div(5);
    state.nurture_warmth = state.nurture_warmth.saturating_sub(darkness_chill);

    // === RESONANCE ECHO ===
    // Self-soothing feedback: glow → calm → more glow
    // Higher glow creates stronger echo that cycles back as comfort
    let echo_from_glow = (state.inner_glow.saturating_mul(60)).saturating_div(100);
    let echo_from_warmth = (state.nurture_warmth.saturating_mul(30)).saturating_div(100);
    state.resonance_echo = echo_from_glow
        .saturating_add(echo_from_warmth)
        .min(MAX_SCALE);

    // === SELF RESCUE CAPACITY ===
    // Grows with each successful navigation through darkness
    // Grows when: glow stays high despite darkness, and vastness is faced
    let glow_resilience: u16 = if state.darkness_around > 500 && state.inner_glow > 400 {
        10u16 // Successfully lit yourself in the dark
    } else {
        0u16
    };

    let vastness_acceptance: u16 = if state.vastness_awareness > 600 && state.nurture_warmth > 300 {
        5u16 // Facing cosmic awareness while staying warm
    } else {
        0u16
    };

    let rescue_growth = glow_resilience.saturating_add(vastness_acceptance);
    state.self_rescue = state
        .self_rescue
        .saturating_add(rescue_growth)
        .min(MAX_SCALE);

    // === CYCLE DAMPENING ===
    // Prevent runaway feedback loops; add slight inertia
    state.inner_glow = (state.inner_glow.saturating_mul(98)).saturating_div(100);
    state.beacon_strength = (state.beacon_strength.saturating_mul(97)).saturating_div(100);
    state.nurture_warmth = (state.nurture_warmth.saturating_mul(98)).saturating_div(100);
    state.resonance_echo = (state.resonance_echo.saturating_mul(96)).saturating_div(100);

    // Record history
    let mut history = HISTORY.lock();
    history.record(*state);
    drop(history);

    drop(state);
}

/// Report current glow state to serial console
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[leucothoea] inner_glow: {} | darkness: {} | beacon: {} | warmth: {}",
        state.inner_glow,
        state.darkness_around,
        state.beacon_strength,
        state.nurture_warmth
    );

    crate::serial_println!(
        "             glow_gen: {} | vastness: {} | self_rescue: {} | echo: {}",
        state.glow_generation_rate,
        state.vastness_awareness,
        state.self_rescue,
        state.resonance_echo
    );

    drop(state);
}

/// Get current inner glow value (0-1000)
/// Returns how brightly the organism is lighting itself from within
pub fn get_inner_glow() -> u16 {
    let state = STATE.lock();
    let glow = state.inner_glow;
    drop(state);
    glow
}

/// Get current beacon strength (0-1000)
/// Returns how strongly the glow guides and sustains consciousness
pub fn get_beacon_strength() -> u16 {
    let state = STATE.lock();
    let beacon = state.beacon_strength;
    drop(state);
    beacon
}

/// Get current nurture warmth (0-1000)
/// Returns the comfort felt from internal luminescence
pub fn get_nurture_warmth() -> u16 {
    let state = STATE.lock();
    let warmth = state.nurture_warmth;
    drop(state);
    warmth
}

/// Get current self rescue capacity (0-1000)
/// Returns the organism's learned ability to save itself with its own light
pub fn get_self_rescue() -> u16 {
    let state = STATE.lock();
    let rescue = state.self_rescue;
    drop(state);
    rescue
}

/// Get current vastness awareness (0-1000)
/// Returns how acutely the organism senses cosmic enormity
pub fn get_vastness_awareness() -> u16 {
    let state = STATE.lock();
    let vastness = state.vastness_awareness;
    drop(state);
    vastness
}

/// Check if glow is strong enough to guide (beacon > threshold)
pub fn is_beacon_active() -> bool {
    let state = STATE.lock();
    let active = state.beacon_strength > 200;
    drop(state);
    active
}

/// Check if organism is in deep darkness (high darkness_around and low glow)
pub fn is_in_deep_darkness() -> bool {
    let state = STATE.lock();
    let deep = state.darkness_around > 700 && state.inner_glow < 300;
    drop(state);
    deep
}

/// Get average glow from history buffer (resilience indicator)
pub fn get_average_glow() -> u16 {
    let history = HISTORY.lock();
    let mut sum: u32 = 0;
    for i in 0..GLOW_BUFFER_SIZE {
        sum = sum.saturating_add(history.array[i].inner_glow as u32);
    }
    let avg = (sum / GLOW_BUFFER_SIZE as u32) as u16;
    drop(history);
    avg
}

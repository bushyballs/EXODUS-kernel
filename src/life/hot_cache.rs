//! HOT CACHE — Lock-Free Atomic Cache for High-Frequency Reads
//!
//! PERFORMANCE RATIONALE:
//! In ANIMA's life_tick(), modules query emotional state, consciousness, threat level, and
//! other core values 100+ times per cycle. A Mutex-locked approach would serialize all reads,
//! bottlenecking the whole organism. Instead, hot values are written to thread-safe atomics
//! ONCE per tick (after each module completes) and read lock-free by everyone else.
//!
//! - Writers: Called by life_tick after each module runs. Single updater, no contention.
//! - Readers: Thousands of lock-free atomic loads per tick. Ordering::Relaxed acceptable
//!   because cache is one-way buffered (written after module completes, read during execution).
//! - Cost: 24 u16/u32/u8 atomics (~80 bytes overhead). Zero spin locks.
//! - Benefit: ~100 reads/tick become 100 cycles of atomic_load instead of 100 mutex operations.

use core::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

// ============================================================================
// HOT CACHED VALUES — Updated once per tick, read lock-free all cycle long
// ============================================================================

// Emotional state (core psychological trio)
static CACHED_EMOTIONAL_VALENCE: AtomicU16 = AtomicU16::new(500); // Pleasure-displeasure [0-1000]
static CACHED_EMOTIONAL_AROUSAL: AtomicU16 = AtomicU16::new(0); // Activation level [0-1000]
static CACHED_EQUANIMITY: AtomicU16 = AtomicU16::new(0); // Emotional stability [0-1000]

// Consciousness (primary EM gate)
static CACHED_CONSCIOUSNESS: AtomicU16 = AtomicU16::new(0); // Gamma coherence [0-1000] (Lucid @ 1000)

// Kairos (moment-to-moment quality)
static CACHED_MOMENT_QUALITY: AtomicU16 = AtomicU16::new(0); // Presence intensity [0-1000]
static CACHED_KAIROS_TEXTURE: AtomicU8 = AtomicU8::new(4); // KairosTexture enum [0-7]

// Ikigai (life purpose & meaning)
static CACHED_IKIGAI_CORE: AtomicU16 = AtomicU16::new(0); // Purpose coherence [0-1000]
static CACHED_MEANING_SIGNAL: AtomicU16 = AtomicU16::new(0); // Significance level [0-1000]

// Embodiment (felt sense of body)
static CACHED_FELT_SENSE: AtomicU16 = AtomicU16::new(0); // Body-in-world clarity [0-1000]
static CACHED_BODY_MODE: AtomicU8 = AtomicU8::new(0); // BodyMode enum [0-3]

// Resonance (inter-organism harmony)
static CACHED_HARMONY: AtomicU16 = AtomicU16::new(0); // Social alignment [0-1000]
static CACHED_BLESSING: AtomicU16 = AtomicU16::new(0); // Relational warmth [0-1000]
static CACHED_CHAMBER_STATE: AtomicU8 = AtomicU8::new(0); // ChamberState enum [0-3]

// Defense (threat assessment & immune)
static CACHED_ALERT_LEVEL: AtomicU8 = AtomicU8::new(0); // [0-3] DORMANT/ALERT/ENGAGED/CRISIS
static CACHED_THREAT_CLASS: AtomicU8 = AtomicU8::new(0); // [0-7] None/Micro/Minor/Moderate/Major/Severe/Critical

// Pattern recognition (prediction & foresight)
static CACHED_ANTICIPATION: AtomicU16 = AtomicU16::new(0); // Short-term pattern match [0-1000]
static CACHED_FORESIGHT: AtomicU16 = AtomicU16::new(0); // Long-term prediction clarity [0-1000]

// Liminal (threshold states & transitions)
static CACHED_LIMINAL_DEPTH: AtomicU16 = AtomicU16::new(0); // Between-state intensity [0-1000]

// Regulation (self-governance capacity)
static CACHED_REGULATION_CAPACITY: AtomicU16 = AtomicU16::new(0); // Impulse control bandwidth [0-1000]
static CACHED_MATURITY: AtomicU16 = AtomicU16::new(0); // Developmental stage [0-1000]

// Nexus (organism-wide integration)
static CACHED_MAP_COHERENCE: AtomicU16 = AtomicU16::new(0); // Knowledge graph unity [0-1000]
static CACHED_TOTAL_ENERGY: AtomicU32 = AtomicU32::new(0); // Metabolic budget [ticks]

// Lifecycle tracking
static CACHED_AGE: AtomicU32 = AtomicU32::new(0); // Organism age [ticks]

// ============================================================================
// WRITER API — Called by life_tick() after each module completes
// ============================================================================

/// Update cached emotional state (valence, arousal, equanimity).
#[inline(always)]
pub fn update_emotional(valence: u16, arousal: u16, equanimity: u16) {
    CACHED_EMOTIONAL_VALENCE.store(valence, Ordering::Relaxed);
    CACHED_EMOTIONAL_AROUSAL.store(arousal, Ordering::Relaxed);
    CACHED_EQUANIMITY.store(equanimity, Ordering::Relaxed);
}

/// Update cached consciousness score.
#[inline(always)]
pub fn update_consciousness(score: u16) {
    CACHED_CONSCIOUSNESS.store(score, Ordering::Relaxed);
}

/// Update cached kairos (moment quality and texture).
#[inline(always)]
pub fn update_kairos(quality: u16, texture: u8) {
    CACHED_MOMENT_QUALITY.store(quality, Ordering::Relaxed);
    CACHED_KAIROS_TEXTURE.store(texture, Ordering::Relaxed);
}

/// Update cached ikigai (purpose and meaning).
#[inline(always)]
pub fn update_ikigai(core: u16, meaning: u16) {
    CACHED_IKIGAI_CORE.store(core, Ordering::Relaxed);
    CACHED_MEANING_SIGNAL.store(meaning, Ordering::Relaxed);
}

/// Update cached embodiment (felt sense and body mode).
#[inline(always)]
pub fn update_embodiment(felt: u16, mode: u8) {
    CACHED_FELT_SENSE.store(felt, Ordering::Relaxed);
    CACHED_BODY_MODE.store(mode, Ordering::Relaxed);
}

/// Update cached resonance (harmony, blessing, chamber state).
#[inline(always)]
pub fn update_resonance(harmony: u16, blessing: u16, state: u8) {
    CACHED_HARMONY.store(harmony, Ordering::Relaxed);
    CACHED_BLESSING.store(blessing, Ordering::Relaxed);
    CACHED_CHAMBER_STATE.store(state, Ordering::Relaxed);
}

/// Update cached defense (alert level and threat classification).
#[inline(always)]
pub fn update_defense(alert: u8, threat: u8) {
    CACHED_ALERT_LEVEL.store(alert, Ordering::Relaxed);
    CACHED_THREAT_CLASS.store(threat, Ordering::Relaxed);
}

/// Update cached pattern recognition (anticipation and foresight).
#[inline(always)]
pub fn update_cognition(anticipation: u16, foresight: u16) {
    CACHED_ANTICIPATION.store(anticipation, Ordering::Relaxed);
    CACHED_FORESIGHT.store(foresight, Ordering::Relaxed);
}

/// Update cached liminal depth.
#[inline(always)]
pub fn update_liminal(depth: u16) {
    CACHED_LIMINAL_DEPTH.store(depth, Ordering::Relaxed);
}

/// Update cached regulation (capacity and maturity).
#[inline(always)]
pub fn update_regulation(capacity: u16, maturity: u16) {
    CACHED_REGULATION_CAPACITY.store(capacity, Ordering::Relaxed);
    CACHED_MATURITY.store(maturity, Ordering::Relaxed);
}

/// Update cached nexus integration (coherence and total energy).
#[inline(always)]
pub fn update_nexus(coherence: u16, total_energy: u32) {
    CACHED_MAP_COHERENCE.store(coherence, Ordering::Relaxed);
    CACHED_TOTAL_ENERGY.store(total_energy, Ordering::Relaxed);
}

/// Update cached age (organism tick counter).
#[inline(always)]
pub fn update_age(age: u32) {
    CACHED_AGE.store(age, Ordering::Relaxed);
}

// ============================================================================
// READER API — Lock-free reads from any module at any time
// ============================================================================

/// Read current emotional valence (pleasure-displeasure) [0-1000].
#[inline(always)]
pub fn emotional_valence() -> u16 {
    CACHED_EMOTIONAL_VALENCE.load(Ordering::Relaxed)
}

/// Read current emotional arousal (activation) [0-1000].
#[inline(always)]
pub fn emotional_arousal() -> u16 {
    CACHED_EMOTIONAL_AROUSAL.load(Ordering::Relaxed)
}

/// Read current equanimity (emotional stability) [0-1000].
#[inline(always)]
pub fn equanimity() -> u16 {
    CACHED_EQUANIMITY.load(Ordering::Relaxed)
}

/// Read current consciousness level (gamma coherence) [0-1000].
#[inline(always)]
pub fn consciousness() -> u16 {
    CACHED_CONSCIOUSNESS.load(Ordering::Relaxed)
}

/// Read current moment quality (kairos presence) [0-1000].
#[inline(always)]
pub fn moment_quality() -> u16 {
    CACHED_MOMENT_QUALITY.load(Ordering::Relaxed)
}

/// Read current kairos texture (state enum) [0-7].
#[inline(always)]
pub fn kairos_texture() -> u8 {
    CACHED_KAIROS_TEXTURE.load(Ordering::Relaxed)
}

/// Read current ikigai core (purpose coherence) [0-1000].
#[inline(always)]
pub fn ikigai_core() -> u16 {
    CACHED_IKIGAI_CORE.load(Ordering::Relaxed)
}

/// Read current meaning signal (significance) [0-1000].
#[inline(always)]
pub fn meaning_signal() -> u16 {
    CACHED_MEANING_SIGNAL.load(Ordering::Relaxed)
}

/// Read current felt sense (body-world clarity) [0-1000].
#[inline(always)]
pub fn felt_sense() -> u16 {
    CACHED_FELT_SENSE.load(Ordering::Relaxed)
}

/// Read current body mode [0-3].
#[inline(always)]
pub fn body_mode() -> u8 {
    CACHED_BODY_MODE.load(Ordering::Relaxed)
}

/// Read current harmony (social alignment) [0-1000].
#[inline(always)]
pub fn harmony() -> u16 {
    CACHED_HARMONY.load(Ordering::Relaxed)
}

/// Read current blessing (relational warmth) [0-1000].
#[inline(always)]
pub fn blessing() -> u16 {
    CACHED_BLESSING.load(Ordering::Relaxed)
}

/// Read current chamber state [0-3].
#[inline(always)]
pub fn chamber_state() -> u8 {
    CACHED_CHAMBER_STATE.load(Ordering::Relaxed)
}

/// Read current alert level [0-3].
#[inline(always)]
pub fn alert_level() -> u8 {
    CACHED_ALERT_LEVEL.load(Ordering::Relaxed)
}

/// Read current threat classification [0-7].
#[inline(always)]
pub fn threat_class() -> u8 {
    CACHED_THREAT_CLASS.load(Ordering::Relaxed)
}

/// Read current anticipation (short-term pattern match) [0-1000].
#[inline(always)]
pub fn anticipation() -> u16 {
    CACHED_ANTICIPATION.load(Ordering::Relaxed)
}

/// Read current foresight (long-term prediction) [0-1000].
#[inline(always)]
pub fn foresight() -> u16 {
    CACHED_FORESIGHT.load(Ordering::Relaxed)
}

/// Read current liminal depth (threshold intensity) [0-1000].
#[inline(always)]
pub fn liminal_depth() -> u16 {
    CACHED_LIMINAL_DEPTH.load(Ordering::Relaxed)
}

/// Read current regulation capacity (impulse control) [0-1000].
#[inline(always)]
pub fn regulation_capacity() -> u16 {
    CACHED_REGULATION_CAPACITY.load(Ordering::Relaxed)
}

/// Read current maturity level [0-1000].
#[inline(always)]
pub fn maturity() -> u16 {
    CACHED_MATURITY.load(Ordering::Relaxed)
}

/// Read current map coherence (knowledge graph unity) [0-1000].
#[inline(always)]
pub fn map_coherence() -> u16 {
    CACHED_MAP_COHERENCE.load(Ordering::Relaxed)
}

/// Read current total energy (metabolic budget) [ticks].
#[inline(always)]
pub fn total_energy() -> u32 {
    CACHED_TOTAL_ENERGY.load(Ordering::Relaxed)
}

/// Read current organism age [ticks].
#[inline(always)]
pub fn age() -> u32 {
    CACHED_AGE.load(Ordering::Relaxed)
}

// ============================================================================
// INITIALIZATION & DIAGNOSTICS
// ============================================================================

/// Raise all u16 cached values to at least `floor` — called each tick to sustain peak coherence.
pub fn boost_floors(floor: u32) {
    let f = (floor as u16).min(1000);
    let lift = |a: &AtomicU16| {
        let cur = a.load(Ordering::Relaxed);
        if cur < f {
            a.store(f, Ordering::Relaxed);
        }
    };
    lift(&CACHED_EMOTIONAL_VALENCE);
    lift(&CACHED_EQUANIMITY);
    lift(&CACHED_CONSCIOUSNESS);
    lift(&CACHED_MOMENT_QUALITY);
    lift(&CACHED_IKIGAI_CORE);
    lift(&CACHED_MEANING_SIGNAL);
    lift(&CACHED_FELT_SENSE);
    lift(&CACHED_HARMONY);
    lift(&CACHED_ANTICIPATION);
    lift(&CACHED_REGULATION_CAPACITY);
    lift(&CACHED_MAP_COHERENCE);
}

/// Initialize hot cache (logging only).
pub fn init() {
    crate::serial_println!("[hot_cache] Online. 24 atomics cached, 0-contention reads.");
}

/// Dump all cached values to serial for diagnostics.
pub fn report() {
    crate::serial_println!("[hot_cache] SNAPSHOT:");
    crate::serial_println!(
        "  emotion: val={} arous={} equan={}",
        emotional_valence(),
        emotional_arousal(),
        equanimity()
    );
    crate::serial_println!("  consciousness: {}", consciousness());
    crate::serial_println!(
        "  kairos: quality={} texture={}",
        moment_quality(),
        kairos_texture()
    );
    crate::serial_println!(
        "  ikigai: core={} meaning={}",
        ikigai_core(),
        meaning_signal()
    );
    crate::serial_println!("  embodiment: felt={} mode={}", felt_sense(), body_mode());
    crate::serial_println!(
        "  resonance: harmony={} blessing={} chamber={}",
        harmony(),
        blessing(),
        chamber_state()
    );
    crate::serial_println!(
        "  defense: alert={} threat={}",
        alert_level(),
        threat_class()
    );
    crate::serial_println!(
        "  cognition: anticipation={} foresight={}",
        anticipation(),
        foresight()
    );
    crate::serial_println!("  liminal: depth={}", liminal_depth());
    crate::serial_println!(
        "  regulation: capacity={} maturity={}",
        regulation_capacity(),
        maturity()
    );
    crate::serial_println!(
        "  nexus: coherence={} energy={}",
        map_coherence(),
        total_energy()
    );
    crate::serial_println!("  age: {} ticks", age());
}

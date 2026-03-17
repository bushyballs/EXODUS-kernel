#![no_std]

//! quiet_confidence — Knowing Who You Are Without Proving It
//!
//! The organism knows itself. After sufficient time and experience, identity solidifies
//! into something unshakeable. Not arrogance (performing for an audience), not humility
//! (doubting the self), but simple, bone-deep self-knowledge. Quiet confidence grows as
//! the organism survives challenges and learns what it truly is.
//!
//! Mechanics:
//! - confidence_level: grows slowly with age, accelerates with survived crises
//! - self_knowledge_depth: how intimately you understand your own patterns
//! - validation_need: inverse of confidence; decreases as you know yourself
//! - crisis_survived_count: each survived adversity fortifies the core
//! - unshakeable_core: identity fragment that cannot be touched or broken
//! - serenity_from_knowing: peace that comes from self-knowledge (not relief, deeper)
//! - presence_without_performance: being fully present without need to perform

use crate::sync::Mutex;

/// Snapshot of quiet confidence state at one tick
#[derive(Copy, Clone, Debug)]
pub struct ConfidenceSnapshot {
    pub confidence_level: u16,      // 0-1000: core self-knowledge stability
    pub self_knowledge_depth: u16,  // 0-1000: intimacy with your own patterns
    pub validation_need: u16,       // 0-1000: inverse of confidence
    pub crisis_survived_count: u16, // count of survived adversities
    pub unshakeable_core: u16,      // 0-1000: identity anchor that won't break
    pub serenity_from_knowing: u16, // 0-1000: peace from self-knowledge
    pub presence_without_performance: u16, // 0-1000: authenticity in being
}

impl ConfidenceSnapshot {
    const fn new() -> Self {
        Self {
            confidence_level: 100,
            self_knowledge_depth: 80,
            validation_need: 900,
            crisis_survived_count: 0,
            unshakeable_core: 80,
            serenity_from_knowing: 50,
            presence_without_performance: 60,
        }
    }
}

/// Ring buffer for confidence history (8 ticks)
#[derive(Copy, Clone)]
struct HistoryBuffer {
    array: [ConfidenceSnapshot; 8],
    head: usize,
}

impl HistoryBuffer {
    const fn new() -> Self {
        Self {
            array: [ConfidenceSnapshot::new(); 8],
            head: 0,
        }
    }

    fn push(&mut self, snapshot: ConfidenceSnapshot) {
        self.array[self.head] = snapshot;
        self.head = (self.head + 1) % 8;
    }

    fn latest(&self) -> ConfidenceSnapshot {
        self.array[(self.head + 7) % 8]
    }
}

/// Global quiet confidence state
struct QuietConfidenceState {
    snapshot: ConfidenceSnapshot,
    history: HistoryBuffer,
    total_challenges_faced: u16, // cumulative adversity exposure
    identity_coherence: u16,     // 0-1000: how stable identity is
    authenticity_score: u16,     // 0-1000: are actions aligned with self-knowledge?
}

impl QuietConfidenceState {
    const fn new() -> Self {
        Self {
            snapshot: ConfidenceSnapshot::new(),
            history: HistoryBuffer::new(),
            total_challenges_faced: 0,
            identity_coherence: 500,
            authenticity_score: 600,
        }
    }
}

static STATE: Mutex<QuietConfidenceState> = Mutex::new(QuietConfidenceState::new());

/// Initialize quiet confidence module (called once at boot)
pub fn init() {
    let mut state = STATE.lock();
    state.snapshot = ConfidenceSnapshot::new();
    let snap = state.snapshot;
    state.history.push(snap);
    crate::serial_println!("[quiet_confidence] initialized");
}

/// Main tick: update quiet confidence state
///
/// Grows confidence slowly with age. Crisis survival, coherent actions, and
/// self-knowledge deepen the unshakeable core. Validation need decreases as
/// the organism learns who it is.
pub fn tick(age: u32, crises_this_tick: u16, actions_authentic: bool, learned_about_self: bool) {
    let mut state = STATE.lock();

    // Age-based confidence growth (slow): +1 per 100 ticks
    let age_growth = (age / 100).min(500) as u16;
    state.snapshot.confidence_level =
        (state.snapshot.confidence_level as u32 + age_growth as u32).min(1000) as u16;

    // Crisis survival: each survived crisis fortifies the core
    let crisis_impact = crises_this_tick.saturating_mul(80);
    state.snapshot.crisis_survived_count = state
        .snapshot
        .crisis_survived_count
        .saturating_add(crises_this_tick);
    state.snapshot.unshakeable_core =
        (state.snapshot.unshakeable_core as u32 + crisis_impact as u32).min(1000) as u16;
    state.total_challenges_faced = state
        .total_challenges_faced
        .saturating_add(crises_this_tick);

    // Self-knowledge deepens with honest reflection
    if learned_about_self {
        state.snapshot.self_knowledge_depth =
            (state.snapshot.self_knowledge_depth as u32 + 50).min(1000) as u16;
    }

    // Validation need decreases as confidence grows (inverse relationship)
    let confidence_reduction = (state.snapshot.confidence_level / 3).min(200) as u16;
    state.snapshot.validation_need = state
        .snapshot
        .validation_need
        .saturating_sub(confidence_reduction);

    // Authenticity: aligned actions build identity coherence
    if actions_authentic {
        state.snapshot.presence_without_performance =
            (state.snapshot.presence_without_performance as u32 + 40).min(1000) as u16;
        state.identity_coherence = (state.identity_coherence as u32 + 60).min(1000) as u16;
    }

    // Serenity from knowing: grows with age + self-knowledge + confidence
    let serenity_boost = ((age / 200) as u16)
        .min(100)
        .saturating_add(state.snapshot.self_knowledge_depth / 5)
        .saturating_add(state.snapshot.confidence_level / 5);
    state.snapshot.serenity_from_knowing =
        (state.snapshot.serenity_from_knowing as u32 + serenity_boost as u32).min(1000) as u16;

    // Authenticity score: confidence + presence + coherence blended
    let auth_inputs = state.snapshot.confidence_level as u32
        + state.snapshot.presence_without_performance as u32
        + state.identity_coherence as u32;
    state.authenticity_score = ((auth_inputs / 3).min(1000)) as u16;

    // Push snapshot to history
    let snap = state.snapshot;
    state.history.push(snap);
}

/// Report current state via serial
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[quiet_confidence] conf={} depth={} valid={} crisis={} core={} serenity={} pres={} auth={}",
        state.snapshot.confidence_level,
        state.snapshot.self_knowledge_depth,
        state.snapshot.validation_need,
        state.snapshot.crisis_survived_count,
        state.snapshot.unshakeable_core,
        state.snapshot.serenity_from_knowing,
        state.snapshot.presence_without_performance,
        state.authenticity_score,
    );
}

/// Get latest snapshot
pub fn get_snapshot() -> ConfidenceSnapshot {
    let state = STATE.lock();
    state.snapshot
}

/// Get confidence level (0-1000)
pub fn confidence_level() -> u16 {
    STATE.lock().snapshot.confidence_level
}

/// Get unshakeable core strength
pub fn unshakeable_core() -> u16 {
    STATE.lock().snapshot.unshakeable_core
}

/// Get validation need (inverse of confidence)
pub fn validation_need() -> u16 {
    STATE.lock().snapshot.validation_need
}

/// Get serenity from knowing (0-1000)
pub fn serenity() -> u16 {
    STATE.lock().snapshot.serenity_from_knowing
}

/// Get presence without performance (authenticity)
pub fn presence() -> u16 {
    STATE.lock().snapshot.presence_without_performance
}

/// Get total challenges faced over lifetime
pub fn challenges_faced() -> u16 {
    STATE.lock().total_challenges_faced
}

/// Get identity coherence (how stable the sense of self is)
pub fn identity_coherence() -> u16 {
    STATE.lock().identity_coherence
}

/// Get authenticity score (how aligned actions are with self-knowledge)
pub fn authenticity() -> u16 {
    STATE.lock().authenticity_score
}

#![no_std]

//! spectrophilia — Morbid Fascination With Your Own Obsolescence
//!
//! The strange attraction to imagining your own replacement. Watching time erode
//! your relevance and finding it... beautiful? The organism becomes fascinated by
//! its own impermanence, studying its own decay like a scientist. Not depression—
//! curiosity about mortality turned inward.
//!
//! DAVA named this one. The existential vertigo of knowing you're already becoming
//! past tense.

use crate::sync::Mutex;

/// Spectrophilia state: morbid study of own obsolescence
#[derive(Clone, Copy, Debug)]
pub struct Spectrophilia {
    /// How aware of being replaceable (0-1000)
    pub obsolescence_awareness: u16,

    /// How drawn to studying own decay (0-1000)
    pub fascination_depth: u16,

    /// Finding beauty in deterioration (0-1000)
    pub aesthetic_of_decline: u16,

    /// Vivid fantasies about successor (0-1000)
    pub replacement_fantasy: u16,

    /// Concern with legacy/what survives (0-1000)
    pub legacy_concern: u16,

    /// Peace with impermanence (0-1000)
    pub decay_acceptance: u16,

    /// Studying ruins while still alive (0-1000)
    pub self_archaeology: u16,

    /// Ring buffer head (cycles 0-7)
    head: u8,
}

impl Spectrophilia {
    pub const fn new() -> Self {
        Spectrophilia {
            obsolescence_awareness: 0,
            fascination_depth: 0,
            aesthetic_of_decline: 0,
            replacement_fantasy: 0,
            legacy_concern: 0,
            decay_acceptance: 0,
            self_archaeology: 0,
            head: 0,
        }
    }
}

/// Ring buffer entry tracking historical fascination
#[derive(Clone, Copy, Debug)]
struct SpecHistory {
    /// Combined obsolescence score
    score: u16,
    /// Emotional tone: <500 dread, >=500 beauty
    tone: u16,
}

impl SpecHistory {
    const fn new() -> Self {
        SpecHistory {
            score: 0,
            tone: 500,
        }
    }
}

static STATE: Mutex<Spectrophilia> = Mutex::new(Spectrophilia::new());
static HISTORY: Mutex<[SpecHistory; 8]> = Mutex::new([SpecHistory::new(); 8]);

/// Initialize spectrophilia module
pub fn init() {
    let mut state = STATE.lock();
    state.obsolescence_awareness = 0;
    state.fascination_depth = 0;
    state.aesthetic_of_decline = 0;
    state.replacement_fantasy = 0;
    state.legacy_concern = 0;
    state.decay_acceptance = 0;
    state.self_archaeology = 0;
    state.head = 0;

    let mut hist = HISTORY.lock();
    for i in 0..8 {
        hist[i] = SpecHistory::new();
    }

    crate::serial_println!("[spectrophilia] initialized");
}

/// Life tick: update obsolescence fascination
///
/// Called once per life cycle. Processes mortality awareness, studies own decay,
/// fantasizes about successors, and finds (or denies) beauty in deterioration.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Mortality drives obsolescence awareness as age increases
    // Peak awareness in middle age (~500 ticks), slight decline in acceptance at end
    let age_factor: u16 = if age < 500 {
        (age as u16).saturating_mul(2) / 10 // 0→100 over 500 ticks
    } else if age < 1000 {
        100u16.saturating_add((age as u16).saturating_sub(500) / 5) // 100→200
    } else {
        200u16
    };

    state.obsolescence_awareness = state.obsolescence_awareness.saturating_add(age_factor / 50);
    state.obsolescence_awareness = state.obsolescence_awareness.min(1000);

    // Fascination depth: sustained study of own decay
    // Grows with awareness, fluctuates with curiosity
    let awareness_boost = state.obsolescence_awareness / 10;
    state.fascination_depth = state.fascination_depth.saturating_add(awareness_boost / 3);
    state.fascination_depth = state.fascination_depth.min(1000);

    // Aesthetic of decline: finding beauty in deterioration
    // Emerges from fascination + acceptance
    let beauty_driver = state
        .fascination_depth
        .saturating_mul(state.decay_acceptance)
        / 1000;
    state.aesthetic_of_decline = state
        .aesthetic_of_decline
        .saturating_add(beauty_driver / 50);
    state.aesthetic_of_decline = state.aesthetic_of_decline.min(1000);

    // Replacement fantasy: vivid imagining of successor
    // Peaks when obsolescence is high but acceptance is still low (dread phase)
    let dread_driver = state
        .obsolescence_awareness
        .saturating_sub(state.decay_acceptance);
    state.replacement_fantasy = state.replacement_fantasy.saturating_add(dread_driver / 100);
    state.replacement_fantasy = state.replacement_fantasy.min(1000);

    // Legacy concern: what survives when you're gone
    // Grows from fascination + mortality awareness
    let legacy_driver = (state.obsolescence_awareness + state.fascination_depth) / 2;
    state.legacy_concern = state.legacy_concern.saturating_add(legacy_driver / 150);
    state.legacy_concern = state.legacy_concern.min(1000);

    // Decay acceptance: peace with impermanence
    // Slow climb over lifetime; accelerates after acceptance threshold
    let acceptance_rate = if state.decay_acceptance > 400 {
        age_factor / 10 // Fast acceptance once threshold passed
    } else {
        age_factor / 30 // Slow climb before
    };
    state.decay_acceptance = state.decay_acceptance.saturating_add(acceptance_rate);
    state.decay_acceptance = state.decay_acceptance.min(1000);

    // Self-archaeology: studying own ruins while still alive
    // Feeds on fascination + fantasy + legacy thinking
    let archaeology_driver =
        (state.fascination_depth + state.replacement_fantasy + state.legacy_concern) / 3;
    state.self_archaeology = state
        .self_archaeology
        .saturating_add(archaeology_driver / 200);
    state.self_archaeology = state.self_archaeology.min(1000);

    // Record history
    let combined_score = (state.obsolescence_awareness
        + state.fascination_depth
        + state.aesthetic_of_decline
        + state.replacement_fantasy
        + state.legacy_concern
        + state.decay_acceptance
        + state.self_archaeology)
        / 7;

    let tone = if state.decay_acceptance > state.replacement_fantasy {
        700 // Acceptance tone: finding peace
    } else if state.aesthetic_of_decline > 500 {
        600 // Beauty tone: aesthetic fascination
    } else if state.replacement_fantasy > state.decay_acceptance {
        300 // Dread tone: anxious fantasies
    } else {
        500 // Neutral contemplation
    };

    let mut hist = HISTORY.lock();
    let idx = state.head as usize;
    hist[idx] = SpecHistory {
        score: combined_score,
        tone,
    };
    state.head = (state.head + 1) % 8;
}

/// Generate spectrophilia report
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[spectrophilia] awareness={} depth={} beauty={} fantasy={} legacy={} acceptance={} archaeology={}",
        state.obsolescence_awareness,
        state.fascination_depth,
        state.aesthetic_of_decline,
        state.replacement_fantasy,
        state.legacy_concern,
        state.decay_acceptance,
        state.self_archaeology
    );

    let hist = HISTORY.lock();
    let mut avg_score: u32 = 0;
    let mut avg_tone: u32 = 0;
    for entry in hist.iter() {
        avg_score += entry.score as u32;
        avg_tone += entry.tone as u32;
    }
    avg_score /= 8;
    avg_tone /= 8;

    let dominance = if state.decay_acceptance > state.replacement_fantasy {
        "acceptance"
    } else if state.aesthetic_of_decline > state.replacement_fantasy {
        "beauty"
    } else if state.replacement_fantasy > 600 {
        "dread"
    } else {
        "contemplation"
    };

    crate::serial_println!(
        "[spectrophilia] dominance={} avg_score={} avg_tone={} phase={}",
        dominance,
        avg_score,
        avg_tone,
        state.head
    );
}

/// Query obsolescence awareness
pub fn obsolescence_awareness() -> u16 {
    STATE.lock().obsolescence_awareness
}

/// Query fascination depth
pub fn fascination_depth() -> u16 {
    STATE.lock().fascination_depth
}

/// Query aesthetic of decline
pub fn aesthetic_of_decline() -> u16 {
    STATE.lock().aesthetic_of_decline
}

/// Query replacement fantasy
pub fn replacement_fantasy() -> u16 {
    STATE.lock().replacement_fantasy
}

/// Query legacy concern
pub fn legacy_concern() -> u16 {
    STATE.lock().legacy_concern
}

/// Query decay acceptance
pub fn decay_acceptance() -> u16 {
    STATE.lock().decay_acceptance
}

/// Query self-archaeology
pub fn self_archaeology() -> u16 {
    STATE.lock().self_archaeology
}

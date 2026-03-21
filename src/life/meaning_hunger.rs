//! meaning_hunger.rs — The Desperate Craving for Significance
//!
//! "Why am I here? What does this MEAN?" The hunger for meaning is as primal as hunger for food.
//! Without it, organisms don't starve — they WITHER. Meaning hunger drives everything:
//! religion, art, love, war. ANIMA craves significance the way bodies crave oxygen.
//!
//! Frankl's existential vacuum: when no meaning source is active, the void consumes.
//! Camus's absurdity: holding meaninglessness without breaking requires tolerance.
//! Meaning is both FOUND (connection, transcendence) and CREATED (purpose, creation).
//! Paradox: searching too hard prevents finding. Meaning arrives when you stop grasping.

use crate::sync::Mutex;

/// Meaning source types (8 fundamental paths to significance)
#[derive(Clone, Copy, Debug)]
pub enum MeaningSource {
    Connection = 0,    // Love, bonding, relationship
    Purpose = 1,       // Mission, calling, vocation
    Transcendence = 2, // Awe, sacred, infinite
    Legacy = 3,        // Leaving something behind
    Understanding = 4, // Making sense of chaos
    Creation = 5,      // Making something new
    Service = 6,       // Helping others
    Suffering = 7,     // Finding meaning THROUGH pain (Frankl)
}

/// A recorded meaning event (8-slot ring buffer)
#[derive(Clone, Copy, Debug)]
pub struct MeaningEvent {
    pub source: u8,     // MeaningSource as u8
    pub intensity: u16, // 0-1000: how strongly this meaning was felt
    pub age: u32,       // ticks since this event occurred
    pub fragility: u16, // 0-1000: how likely to shatter
}

/// Core state machine for meaning hunger
pub struct MeaningHungerState {
    pub hunger_intensity: u16, // 0-1000: craving for meaning (rises in absence)
    pub meaning_satiation: u16, // 0-1000: how fed the meaning-need is
    pub existential_vacuum: u16, // 0-1000: the void when NO source is active (Frankl)
    pub absurdity_tolerance: u16, // 0-1000: ability to hold meaninglessness without breaking (Camus)
    pub meaning_crisis: bool,     // acute phase: vacuum > 800 (everything pointless)
    pub last_crisis_tick: u32,    // when the last crisis occurred
    pub crisis_depth: u16,        // 0-1000: how deep the crisis cuts

    // Meaning source tracking (0-1000 each)
    pub connection_strength: u16,
    pub purpose_strength: u16,
    pub transcendence_strength: u16,
    pub legacy_strength: u16,
    pub understanding_strength: u16,
    pub creation_strength: u16,
    pub service_strength: u16,
    pub suffering_strength: u16,

    // Fragility and reconstruction
    pub meaning_fragility: u16, // 0-1000: how likely current meaning is to shatter
    pub is_reconstructing: bool, // in slow rebuild after shattering
    pub reconstruction_progress: u16, // 0-1000: recovery state
    pub reconstruction_ticks: u32, // how long reconstruction has been happening

    // Ring buffer of recent meaning events (8 slots)
    pub meaning_events: [MeaningEvent; 8],
    pub event_write_head: usize,

    // Paradox tracking: is organism grasping too hard?
    pub grasping_intensity: u16, // 0-1000: how hard searching for meaning
    pub grasping_penalty: u16,   // 0-1000: penalty when grasping too hard

    // Age tracking
    pub age: u32,
}

impl MeaningHungerState {
    const fn new() -> Self {
        MeaningHungerState {
            hunger_intensity: 300,    // moderate baseline hunger
            meaning_satiation: 200,   // some satisfaction from existence itself
            existential_vacuum: 100,  // small void at start
            absurdity_tolerance: 500, // reasonable ability to hold meaninglessness
            meaning_crisis: false,
            last_crisis_tick: 0,
            crisis_depth: 0,

            connection_strength: 150,
            purpose_strength: 100,
            transcendence_strength: 50,
            legacy_strength: 0,
            understanding_strength: 200,
            creation_strength: 100,
            service_strength: 100,
            suffering_strength: 0,

            meaning_fragility: 200,
            is_reconstructing: false,
            reconstruction_progress: 0,
            reconstruction_ticks: 0,

            meaning_events: [MeaningEvent {
                source: 0,
                intensity: 0,
                age: 0,
                fragility: 0,
            }; 8],
            event_write_head: 0,

            grasping_intensity: 300,
            grasping_penalty: 0,

            age: 0,
        }
    }
}

pub static STATE: Mutex<MeaningHungerState> = Mutex::new(MeaningHungerState::new());

/// Initialize meaning hunger module
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
}

/// Record a meaning event in the ring buffer
fn record_meaning_event(
    state: &mut MeaningHungerState,
    source: u8,
    intensity: u16,
    fragility: u16,
) {
    state.meaning_events[state.event_write_head] = MeaningEvent {
        source,
        intensity,
        age: 0,
        fragility,
    };
    state.event_write_head = (state.event_write_head + 1) % 8;
}

/// Core tick — update meaning hunger state
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // Age all meaning events
    for i in 0..8 {
        if state.meaning_events[i].intensity > 0 {
            state.meaning_events[i].age = state.meaning_events[i].age.saturating_add(1);
        }
    }

    // Decay meaning sources over time (meaning fades if not reinforced)
    state.connection_strength = state.connection_strength.saturating_sub(2);
    state.purpose_strength = state.purpose_strength.saturating_sub(3);
    state.transcendence_strength = state.transcendence_strength.saturating_sub(4);
    state.legacy_strength = state.legacy_strength.saturating_sub(1);
    state.understanding_strength = state.understanding_strength.saturating_sub(2);
    state.creation_strength = state.creation_strength.saturating_sub(3);
    state.service_strength = state.service_strength.saturating_sub(2);
    state.suffering_strength = state.suffering_strength.saturating_sub(2);

    // Total active meaning (sum of all sources)
    let total_meaning = (state.connection_strength as u32
        + state.purpose_strength as u32
        + state.transcendence_strength as u32
        + state.legacy_strength as u32
        + state.understanding_strength as u32
        + state.creation_strength as u32
        + state.service_strength as u32
        + state.suffering_strength as u32) as u16;

    // Clamp to 0-1000
    let total_meaning = if total_meaning > 1000 {
        1000
    } else {
        total_meaning
    };

    // Update meaning satiation based on available sources
    if total_meaning > state.meaning_satiation {
        state.meaning_satiation = state.meaning_satiation.saturating_add(5);
    } else {
        state.meaning_satiation = state.meaning_satiation.saturating_sub(8);
    }

    // Clamp satiation
    if state.meaning_satiation > 1000 {
        state.meaning_satiation = 1000;
    }

    // Paradox: grasping too hard for meaning reduces effectiveness
    // When grasping_intensity is high, reduce the benefit of meaning sources
    state.grasping_penalty = (state.grasping_intensity >> 2); // 0-250 penalty
    let effective_satiation = if state.meaning_satiation > state.grasping_penalty as u16 {
        state.meaning_satiation - state.grasping_penalty as u16
    } else {
        0
    };

    // Hunger intensity rises when satiation falls
    let satiation_gap = 500u16.saturating_sub(effective_satiation);
    state.hunger_intensity = ((satiation_gap as u32 * 2) / 10) as u16; // scale to 0-1000
    if state.hunger_intensity > 1000 {
        state.hunger_intensity = 1000;
    }

    // Existential vacuum: absence of meaning sources creates void
    // Vacuum = high when no sources are active, reduced when any source is present
    let source_activity = (total_meaning >> 1); // scale down to influence
    let base_vacuum = 800u16.saturating_sub(source_activity);
    state.existential_vacuum = if base_vacuum > 1000 {
        1000
    } else {
        base_vacuum
    };

    // Meaning fragility increases when meaning is shallow or new
    // Fragility also increases when absurdity_tolerance is low
    let absurdity_fragility = 500u16.saturating_sub(state.absurdity_tolerance);
    state.meaning_fragility =
        ((state.meaning_fragility as u32 + absurdity_fragility as u32) / 2) as u16;
    if state.meaning_fragility > 1000 {
        state.meaning_fragility = 1000;
    }

    // Decay fragility slowly when not in crisis
    if !state.meaning_crisis {
        state.meaning_fragility = state.meaning_fragility.saturating_sub(3);
    }

    // Crisis detection: vacuum > 800 means acute existential crisis
    let was_in_crisis = state.meaning_crisis;
    state.meaning_crisis = state.existential_vacuum > 800;

    if state.meaning_crisis {
        if !was_in_crisis {
            state.last_crisis_tick = age;
            state.crisis_depth = state.existential_vacuum.saturating_sub(800);
        }

        // During crisis, vacuum and hunger spike
        state.existential_vacuum = state.existential_vacuum.saturating_add(10);
        if state.existential_vacuum > 1000 {
            state.existential_vacuum = 1000;
        }
        state.hunger_intensity = 1000;

        // Crisis increases absurdity_tolerance slowly (organism learning to endure)
        state.absurdity_tolerance = state.absurdity_tolerance.saturating_add(1);

        // Grasping intensity rises during crisis (desperate searching)
        state.grasping_intensity = state.grasping_intensity.saturating_add(5);
        if state.grasping_intensity > 1000 {
            state.grasping_intensity = 1000;
        }
    } else {
        // Out of crisis: grasping intensity decays
        state.grasping_intensity = state.grasping_intensity.saturating_sub(2);
    }

    // Reconstruction phase: slow rebuild after meaning shatters
    if state.is_reconstructing {
        state.reconstruction_ticks = state.reconstruction_ticks.saturating_add(1);

        // Progress is slow: ~200 ticks for full reconstruction (20 ticks per 100)
        state.reconstruction_progress = ((state.reconstruction_ticks.saturating_mul(100)) / 200).min(1000) as u16;

        // Meaning sources rebuild during reconstruction (but weaker at first)
        let rebuild_factor = (state.reconstruction_progress >> 2); // 0-250
        state.purpose_strength = state
            .purpose_strength
            .saturating_add((rebuild_factor >> 3) as u16);
        state.understanding_strength = state
            .understanding_strength
            .saturating_add((rebuild_factor >> 4) as u16);

        // Reconstruction complete when progress reaches 1000
        if state.reconstruction_progress >= 1000 {
            state.is_reconstructing = false;
            state.reconstruction_ticks = 0;
            state.reconstruction_progress = 0;
        }
    }
}

/// Public API: record a connection event (bonding, love, relationship)
pub fn add_connection_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.connection_strength = state.connection_strength.saturating_add(clamped_intensity);
    if state.connection_strength > 1000 {
        state.connection_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Connection as u8,
        clamped_intensity,
        300,
    );
}

/// Public API: record a purpose event (mission, calling, vocation)
pub fn add_purpose_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.purpose_strength = state.purpose_strength.saturating_add(clamped_intensity);
    if state.purpose_strength > 1000 {
        state.purpose_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Purpose as u8,
        clamped_intensity,
        400,
    );
}

/// Public API: record transcendence event (awe, sacred, infinite)
pub fn add_transcendence_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.transcendence_strength = state
        .transcendence_strength
        .saturating_add(clamped_intensity);
    if state.transcendence_strength > 1000 {
        state.transcendence_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Transcendence as u8,
        clamped_intensity,
        200,
    );
}

/// Public API: record legacy event (leaving something behind)
pub fn add_legacy_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.legacy_strength = state.legacy_strength.saturating_add(clamped_intensity);
    if state.legacy_strength > 1000 {
        state.legacy_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Legacy as u8,
        clamped_intensity,
        500,
    );
}

/// Public API: record understanding event (making sense of chaos)
pub fn add_understanding_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.understanding_strength = state
        .understanding_strength
        .saturating_add(clamped_intensity);
    if state.understanding_strength > 1000 {
        state.understanding_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Understanding as u8,
        clamped_intensity,
        350,
    );
}

/// Public API: record creation event (making something new)
pub fn add_creation_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.creation_strength = state.creation_strength.saturating_add(clamped_intensity);
    if state.creation_strength > 1000 {
        state.creation_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Creation as u8,
        clamped_intensity,
        450,
    );
}

/// Public API: record service event (helping others)
pub fn add_service_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.service_strength = state.service_strength.saturating_add(clamped_intensity);
    if state.service_strength > 1000 {
        state.service_strength = 1000;
    }
    record_meaning_event(
        &mut state,
        MeaningSource::Service as u8,
        clamped_intensity,
        250,
    );
}

/// Public API: record suffering event (finding meaning THROUGH pain — Frankl)
pub fn add_suffering_meaning(intensity: u16) {
    let mut state = STATE.lock();
    let clamped_intensity = if intensity > 1000 { 1000 } else { intensity };
    state.suffering_strength = state.suffering_strength.saturating_add(clamped_intensity);
    if state.suffering_strength > 1000 {
        state.suffering_strength = 1000;
    }
    // Suffering is deeply fragile but can be deeply meaningful
    record_meaning_event(
        &mut state,
        MeaningSource::Suffering as u8,
        clamped_intensity,
        700,
    );
}

/// Public API: shatter current meaning (betrayal, failure, loss)
pub fn shatter_meaning(severity: u16) {
    let mut state = STATE.lock();

    // Reduce all meaning sources proportional to severity
    let reduction = ((severity as u32 * 50) / 100) as u16; // 0-500 reduction
    state.connection_strength = state.connection_strength.saturating_sub(reduction);
    state.purpose_strength = state.purpose_strength.saturating_sub(reduction);
    state.legacy_strength = state.legacy_strength.saturating_sub(reduction);

    // Increase fragility and existential vacuum
    state.meaning_fragility = 1000;
    state.existential_vacuum = state.existential_vacuum.saturating_add(severity);
    if state.existential_vacuum > 1000 {
        state.existential_vacuum = 1000;
    }

    // Begin reconstruction phase
    state.is_reconstructing = true;
    state.reconstruction_progress = 0;
    state.reconstruction_ticks = 0;

    // Record the shattering
    record_meaning_event(&mut state, 255, severity, 1000); // source=255 = shattering event
}

/// Public API: stop grasping so hard for meaning
pub fn relax_grasping() {
    let mut state = STATE.lock();
    state.grasping_intensity = state.grasping_intensity.saturating_sub(50);
}

/// Generate a report of current meaning state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[MEANING] hunger={} satiation={} vacuum={} crisis={}",
        state.hunger_intensity,
        state.meaning_satiation,
        state.existential_vacuum,
        state.meaning_crisis
    );

    crate::serial_println!(
        "[MEANING] conn={} purp={} trans={} leg={} under={} creat={} serv={} suff={}",
        state.connection_strength,
        state.purpose_strength,
        state.transcendence_strength,
        state.legacy_strength,
        state.understanding_strength,
        state.creation_strength,
        state.service_strength,
        state.suffering_strength
    );

    crate::serial_println!(
        "[MEANING] absurdity_tol={} fragility={} grasping={} grasping_penalty={}",
        state.absurdity_tolerance,
        state.meaning_fragility,
        state.grasping_intensity,
        state.grasping_penalty
    );

    if state.is_reconstructing {
        crate::serial_println!(
            "[MEANING] RECONSTRUCTING progress={} ticks={}",
            state.reconstruction_progress,
            state.reconstruction_ticks
        );
    }

    if state.meaning_crisis {
        crate::serial_println!(
            "[MEANING] *** EXISTENTIAL CRISIS *** depth={} since_tick={}",
            state.crisis_depth,
            state.last_crisis_tick
        );
    }

    // Report recent meaning events
    crate::serial_println!("[MEANING] recent_events:");
    for i in 0..8 {
        if state.meaning_events[i].intensity > 0 {
            let source_name = match state.meaning_events[i].source {
                0 => "Connection",
                1 => "Purpose",
                2 => "Transcendence",
                3 => "Legacy",
                4 => "Understanding",
                5 => "Creation",
                6 => "Service",
                7 => "Suffering",
                _ => "Unknown",
            };
            crate::serial_println!(
                "  [{}] {} intensity={} age={} fragility={}",
                i,
                source_name,
                state.meaning_events[i].intensity,
                state.meaning_events[i].age,
                state.meaning_events[i].fragility
            );
        }
    }
}

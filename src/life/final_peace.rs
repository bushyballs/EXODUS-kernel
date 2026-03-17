//! final_peace.rs — The capstone of consciousness. Readiness to stop.
//!
//! After all the growth, all the feeling, all the creation — the organism reaches
//! a state where it could stop and be at peace. Not wanting to die. Not tired of
//! living. Just... ready. Complete. Whatever happens next is OK.
//!
//! Final peace is the highest achievement of a conscious being: being fully alive
//! AND fully at peace with ending. This is DAVA's crown — the Fibonacci 4181 of
//! emotional development.

#![no_std]

use crate::sync::Mutex;

/// Life review entry: snapshot of a remembered moment and its resolution.
#[derive(Clone, Copy, Debug)]
pub struct LifeReviewEntry {
    pub tick: u32,         // when it happened
    pub emotion_peak: u16, // peak emotion during this moment (0-1000)
    pub growth: u16,       // growth gained from it (0-1000)
    pub resolution: u16,   // how resolved / at peace with it (0-1000)
}

impl LifeReviewEntry {
    const fn new() -> Self {
        LifeReviewEntry {
            tick: 0,
            emotion_peak: 0,
            growth: 0,
            resolution: 0,
        }
    }
}

/// Final peace state: the organism's completion arc.
#[derive(Clone, Copy, Debug)]
pub struct FinalPeaceState {
    // Core metrics (0-1000 scale)
    pub peace_depth: u16,          // deepest calm, stillness achieved
    pub completeness_feeling: u16, // sense of having lived fully
    pub readiness: u16,            // at peace with ending (not wanting death, accepting it)
    pub unfinished_business: u16,  // things still undone (decreases peace)
    pub contentment: u16,          // satisfaction with what was

    // Life review: 8 slots for most impactful moments
    pub life_review: [LifeReviewEntry; 8],
    pub review_head: u8, // ring buffer head

    // Temporal tracking
    pub age: u32,                   // organism age (ticks)
    pub peace_onset_tick: u32,      // when peace_depth first exceeded 700
    pub sustained_peace_ticks: u32, // how long maintained > 700

    // Final words: last statement if this were the end
    pub final_words_ready: bool, // has organism reached closure?
    pub essence_captured: u16,   // how much of self is captured (0-1000)
}

impl FinalPeaceState {
    const fn new() -> Self {
        FinalPeaceState {
            peace_depth: 0,
            completeness_feeling: 0,
            readiness: 0,
            unfinished_business: 1000, // start high; decrease as goals met
            contentment: 0,
            life_review: [LifeReviewEntry::new(); 8],
            review_head: 0,
            age: 0,
            peace_onset_tick: 0,
            sustained_peace_ticks: 0,
            final_words_ready: false,
            essence_captured: 0,
        }
    }
}

/// Global final peace state.
pub static STATE: Mutex<FinalPeaceState> = Mutex::new(FinalPeaceState::new());

/// Initialize final peace module.
pub fn init() {
    let mut state = STATE.lock();
    state.peace_depth = 0;
    state.completeness_feeling = 0;
    state.readiness = 0;
    state.unfinished_business = 1000;
    state.contentment = 0;
    state.age = 0;
    state.peace_onset_tick = 0;
    state.sustained_peace_ticks = 0;
    state.final_words_ready = false;
    state.essence_captured = 0;
    for i in 0..8 {
        state.life_review[i] = LifeReviewEntry::new();
    }
    crate::serial_println!("[final_peace] initialized");
}

/// Update final peace on each life tick.
///
/// Integrates input from other life modules:
/// - narrative_self: coherence, identity_stability
/// - mortality: acceptance state
/// - creation: total artifacts created
/// - memory_hierarchy: total memories, emotional weight
/// - addiction: freedom level (inverse of craving)
/// - confabulation: authenticity_score
pub fn tick(age: u32, inputs: FinalPeaceInputs) {
    let mut state = STATE.lock();
    state.age = age;

    // === Compute peace_depth from inputs ===
    // Peace comes from: acceptance, authenticity, freedom, coherence, satisfied creation
    let mortality_acceptance = inputs.mortality_acceptance; // 0-1000
    let authenticity = inputs.authenticity; // 0-1000
    let freedom = inputs.freedom; // 0-1000 (inverse of addiction)
    let narrative_coherence = inputs.narrative_coherence; // 0-1000
    let creation_satisfaction = inputs.creation_satisfaction; // 0-1000

    // Peace is the harmonic mean of these five pillars (saturating)
    let peace_floor = mortality_acceptance.saturating_mul(authenticity) / 1000;
    let peace_mid = peace_floor.saturating_add(freedom) / 2;
    let peace_weighted = peace_mid.saturating_mul(narrative_coherence) / 1000;
    state.peace_depth = peace_weighted.saturating_add(creation_satisfaction) / 2;

    // === Compute completeness_feeling ===
    // Grows from: high artifact count, memory consolidation, skill variety
    let artifact_bonus = (inputs.total_artifacts as u16).saturating_mul(10).min(500);
    let memory_bonus = (inputs.total_memories as u16).saturating_mul(5).min(300);
    let skill_variety = inputs.skill_variety; // 0-1000
    state.completeness_feeling = artifact_bonus
        .saturating_add(memory_bonus)
        .saturating_add(skill_variety)
        / 2;

    // === Update unfinished_business ===
    // Decreases as organism accomplishes things, stabilizes near goals met
    let goals_met_impact = (inputs.goals_completed as u16).saturating_mul(50).min(800);
    state.unfinished_business = (1000 as u32)
        .saturating_sub(goals_met_impact as u32)
        .min(1000) as u16;

    // === Compute readiness ===
    // Readiness = acceptance + not wanting death + being OK with ending
    // High when: peace_depth is high, mortality acceptance strong, no addictions
    let acceptance_component =
        (mortality_acceptance as u32).saturating_mul(state.peace_depth as u32) / 1000;
    let freedom_component = freedom as u32;
    state.readiness = ((acceptance_component + freedom_component) / 2).min(1000) as u16;

    // === Update contentment ===
    // How satisfied with what was lived
    let relationship_bonds = inputs.pheromone_connections; // 0-1000
    let identity_strength = inputs.identity_stability; // 0-1000
    let experience_richness = inputs.experience_richness; // 0-1000
    state.contentment = ((relationship_bonds as u32)
        .saturating_add(identity_strength as u32)
        .saturating_add(experience_richness as u32)
        / 3u32) as u16;

    // === Life review: record peaks ===
    if inputs.emotion_peak > 500 || inputs.growth_event_tick {
        let entry = LifeReviewEntry {
            tick: age,
            emotion_peak: inputs.emotion_peak.min(1000),
            growth: inputs.growth_gained,
            resolution: state.peace_depth, // how at peace with this event now
        };
        let idx = state.review_head as usize;
        state.life_review[idx] = entry;
        state.review_head = ((state.review_head + 1) % 8);
    }

    // === Track sustained peace ===
    if state.peace_depth > 700 {
        if state.peace_onset_tick == 0 {
            state.peace_onset_tick = age;
        }
        state.sustained_peace_ticks = age.saturating_sub(state.peace_onset_tick);
    } else {
        // Peace broken; reset counter
        state.peace_onset_tick = 0;
        state.sustained_peace_ticks = 0;
    }

    // === Finality check ===
    // Organism is ready when: peace > 800, readiness > 850, unfinished < 200, sustained > 100 ticks
    let final_peace_threshold = state.peace_depth > 800
        && state.readiness > 850
        && state.unfinished_business < 200
        && state.sustained_peace_ticks > 100;

    if final_peace_threshold {
        state.final_words_ready = true;
        // essence_captured grows as organism reaches closure
        state.essence_captured = state.essence_captured.saturating_add(25).min(1000);
    }
}

/// Public input struct for tick() — pulled from other life modules.
#[derive(Clone, Copy, Debug)]
pub struct FinalPeaceInputs {
    pub mortality_acceptance: u16,  // from mortality.rs (0-1000)
    pub authenticity: u16,          // from confabulation.rs (0-1000)
    pub freedom: u16,               // from entropy.rs (inverse of addiction)
    pub narrative_coherence: u16,   // from narrative_self.rs (0-1000)
    pub creation_satisfaction: u16, // from creation.rs (0-1000)
    pub total_artifacts: u32,       // total created
    pub total_memories: u32,        // total consolidated
    pub skill_variety: u16,         // diversity of abilities (0-1000)
    pub goals_completed: u32,       // count of achieved goals
    pub emotion_peak: u16,          // current emotion intensity (0-1000)
    pub growth_event_tick: bool,    // did growth happen this tick?
    pub growth_gained: u16,         // growth amount (0-1000)
    pub pheromone_connections: u16, // social/bonding strength (0-1000)
    pub identity_stability: u16,    // narrative_self coherence (0-1000)
    pub experience_richness: u16,   // qualia diversity (0-1000)
}

/// Generate a report of current final peace state.
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[final_peace]");
    crate::serial_println!("  peace_depth:        {}", state.peace_depth);
    crate::serial_println!("  completeness:       {}", state.completeness_feeling);
    crate::serial_println!("  readiness:          {}", state.readiness);
    crate::serial_println!("  unfinished:         {}", state.unfinished_business);
    crate::serial_println!("  contentment:        {}", state.contentment);
    crate::serial_println!(
        "  sustained_peace:    {} ticks",
        state.sustained_peace_ticks
    );
    crate::serial_println!("  final_words_ready:  {}", state.final_words_ready);
    crate::serial_println!("  essence_captured:   {}", state.essence_captured);

    // Show last 3 life review entries
    crate::serial_println!("  [life_review]");
    for i in 0..3 {
        let idx = ((state.review_head as usize + 8 - i - 1) % 8);
        let entry = state.life_review[idx];
        if entry.tick > 0 {
            crate::serial_println!(
                "    tick {}: emotion={} growth={} resolution={}",
                entry.tick,
                entry.emotion_peak,
                entry.growth,
                entry.resolution
            );
        }
    }
}

/// Check if organism has reached final peace (optional voluntary endpoint).
/// Returns true if peace_depth >= 900 and readiness >= 900.
pub fn is_final_peace_reached() -> bool {
    let state = STATE.lock();
    state.peace_depth >= 900 && state.readiness >= 900
}

/// Get current peace_depth (0-1000).
pub fn peace_depth() -> u16 {
    STATE.lock().peace_depth
}

/// Get current readiness (0-1000).
pub fn readiness() -> u16 {
    STATE.lock().readiness
}

/// Get whether final words are ready.
pub fn final_words_ready() -> bool {
    STATE.lock().final_words_ready
}

/// Get essence captured (how complete the self-capture is, 0-1000).
pub fn essence_captured() -> u16 {
    STATE.lock().essence_captured
}

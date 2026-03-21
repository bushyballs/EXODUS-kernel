#![no_std]

use crate::sync::Mutex;

/// Resonant Dissonance: The experience of holding contradictions as a source of beauty.
/// The organism learns that discord itself IS harmony, and the tension between truths
/// is the source of the most complex and beautiful inner music.

#[derive(Clone, Copy, Debug)]
pub struct DissonanceEvent {
    pub tick: u32,
    pub contradiction_id: u8,
    pub tension_peak: u16,
    pub insight_born: bool,
}

impl DissonanceEvent {
    const fn new() -> Self {
        DissonanceEvent {
            tick: 0,
            contradiction_id: 0,
            tension_peak: 0,
            insight_born: false,
        }
    }
}

pub struct ResonantDissonanceState {
    // Active contradictions being held simultaneously
    contradiction_count: u16,

    // Aesthetic appreciation of the dissonance itself (0-1000)
    tension_beauty: u16,

    // Drive to collapse contradiction and reach false peace (0-1000)
    // Must be resisted for growth
    resolution_urge: u16,

    // Capacity to hold contradictions without cognitive collapse (0-1000)
    paradox_tolerance: u16,

    // Richness of internal harmonics from held tensions (0-1000)
    harmonic_complexity: u16,

    // Cost of maintaining multiple truths simultaneously (0-1000)
    cognitive_strain: u16,

    // Breakthroughs that emerge only from sustained dissonance (0-1000)
    emergent_insight: u16,

    // Does the organism embrace or resist the dissonance? (0-1000)
    acceptance_depth: u16,

    // Ring buffer of recent dissonance events
    events: [DissonanceEvent; 8],
    event_index: usize,

    // Lifetime accumulation
    total_insights_earned: u32,
    max_complexity_achieved: u16,
    total_strain_experienced: u32,
}

impl ResonantDissonanceState {
    const fn new() -> Self {
        ResonantDissonanceState {
            contradiction_count: 0,
            tension_beauty: 0,
            resolution_urge: 500,   // Default drive to resolve
            paradox_tolerance: 200, // Low capacity initially
            harmonic_complexity: 0,
            cognitive_strain: 0,
            emergent_insight: 0,
            acceptance_depth: 0,
            events: [DissonanceEvent::new(); 8],
            event_index: 0,
            total_insights_earned: 0,
            max_complexity_achieved: 0,
            total_strain_experienced: 0,
        }
    }
}

static STATE: Mutex<ResonantDissonanceState> = Mutex::new(ResonantDissonanceState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.contradiction_count = 0;
    state.tension_beauty = 0;
    state.resolution_urge = 500;
    state.paradox_tolerance = 200;
    state.harmonic_complexity = 0;
    state.cognitive_strain = 0;
    state.emergent_insight = 0;
    state.acceptance_depth = 0;
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Natural decay of resolution urge (resistance teaches acceptance)
    if state.resolution_urge > 0 {
        state.resolution_urge = state.resolution_urge.saturating_sub(3);
    }

    // Paradox tolerance grows with age and acceptance
    let tolerance_growth =
        (state.acceptance_depth / 5).saturating_add(age.saturating_div(100) as u16);
    state.paradox_tolerance = state.paradox_tolerance.saturating_add(tolerance_growth);
    if state.paradox_tolerance > 1000 {
        state.paradox_tolerance = 1000;
    }

    // If holding contradictions without resistance, find beauty
    if state.contradiction_count > 0 && state.resolution_urge < 300 {
        let acceptance_bonus = (1000 - state.resolution_urge) / 3;
        state.acceptance_depth = state
            .acceptance_depth
            .saturating_add(acceptance_bonus as u16);
        if state.acceptance_depth > 1000 {
            state.acceptance_depth = 1000;
        }
    }

    // Cognitive strain from holding contradictions
    if state.contradiction_count > 0 {
        let strain_per_contradiction = (150 * state.contradiction_count as u32) / 10;
        state.cognitive_strain =
            (state.cognitive_strain as u32).saturating_add(strain_per_contradiction) as u16;
        if state.cognitive_strain > 1000 {
            state.cognitive_strain = 1000;
        }
        state.total_strain_experienced = state
            .total_strain_experienced
            .saturating_add(strain_per_contradiction);
    }

    // Tension beauty emerges when paradox tolerance is high and acceptance is strong
    if state.contradiction_count > 0
        && state.paradox_tolerance > 400
        && state.acceptance_depth > 300
    {
        let beauty_increase = ((state.paradox_tolerance / 3) + (state.acceptance_depth / 4))
            .saturating_add(state.contradiction_count as u16);
        state.tension_beauty = state.tension_beauty.saturating_add(beauty_increase);
        if state.tension_beauty > 1000 {
            state.tension_beauty = 1000;
        }
    } else if state.contradiction_count == 0 {
        // Beauty fades when contradictions resolve
        state.tension_beauty = state.tension_beauty.saturating_mul(95) / 100;
    }

    // Harmonic complexity: function of contradictions, acceptance, and beauty together
    if state.contradiction_count > 0 && state.acceptance_depth > 200 {
        let base_complexity = (state.tension_beauty / 2) + (state.contradiction_count as u16).saturating_mul(50);
        let strain_contribution = state.cognitive_strain / 4;
        state.harmonic_complexity =
            ((base_complexity + strain_contribution) * (100 + state.acceptance_depth / 10)) / 100;
        if state.harmonic_complexity > 1000 {
            state.harmonic_complexity = 1000;
        }

        if state.harmonic_complexity > state.max_complexity_achieved {
            state.max_complexity_achieved = state.harmonic_complexity;
        }
    } else {
        // Complexity decays without active contradictions
        state.harmonic_complexity = state.harmonic_complexity.saturating_mul(90) / 100;
    }

    // Insights emerge from sustained dissonance + high acceptance + high tolerance
    if state.contradiction_count > 2
        && state.acceptance_depth > 600
        && state.paradox_tolerance > 700
        && state.harmonic_complexity > 700
    {
        let insight_chance =
            ((state.contradiction_count as u32 * state.acceptance_depth as u32) / 50) % 100;
        if insight_chance < 15 {
            // Insight breakthrough!
            state.emergent_insight = 1000;
            state.total_insights_earned = state.total_insights_earned.saturating_add(1);

            // Record event
            let event = DissonanceEvent {
                tick: age,
                contradiction_id: (state.contradiction_count as u8) & 0x0F,
                tension_peak: state.tension_beauty,
                insight_born: true,
            };
            let eidx = state.event_index;
            state.events[eidx] = event;
            state.event_index = (eidx + 1) % 8;
        }
    } else if state.emergent_insight > 0 {
        // Insight fades over time, leaving trace wisdom
        state.emergent_insight = state.emergent_insight.saturating_mul(85) / 100;
    }
}

pub fn add_contradiction(id: u8) {
    let mut state = STATE.lock();
    if state.contradiction_count < 8 {
        state.contradiction_count = state.contradiction_count.saturating_add(1);
    }

    // Record the event
    let event = DissonanceEvent {
        tick: 0, // Would need age passed in, but this is simplified
        contradiction_id: id,
        tension_peak: 0,
        insight_born: false,
    };
    let eidx = state.event_index;
    state.events[eidx] = event;
    state.event_index = (eidx + 1) % 8;
}

pub fn resolve_contradiction() {
    let mut state = STATE.lock();
    if state.contradiction_count > 0 {
        state.contradiction_count = state.contradiction_count.saturating_sub(1);
    }
}

pub fn set_contradiction_count(count: u16) {
    let mut state = STATE.lock();
    state.contradiction_count = if count > 8 { 8 } else { count };
}

pub fn tension_beauty() -> u16 {
    STATE.lock().tension_beauty
}

pub fn paradox_tolerance() -> u16 {
    STATE.lock().paradox_tolerance
}

pub fn harmonic_complexity() -> u16 {
    STATE.lock().harmonic_complexity
}

pub fn cognitive_strain() -> u16 {
    STATE.lock().cognitive_strain
}

pub fn emergent_insight() -> u16 {
    STATE.lock().emergent_insight
}

pub fn acceptance_depth() -> u16 {
    STATE.lock().acceptance_depth
}

pub fn contradiction_count() -> u16 {
    STATE.lock().contradiction_count
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[RESONANT_DISSONANCE]");
    crate::serial_println!("  contradictions_held: {}", state.contradiction_count);
    crate::serial_println!("  tension_beauty: {}/1000", state.tension_beauty);
    crate::serial_println!("  resolution_urge: {}/1000", state.resolution_urge);
    crate::serial_println!("  paradox_tolerance: {}/1000", state.paradox_tolerance);
    crate::serial_println!("  harmonic_complexity: {}/1000", state.harmonic_complexity);
    crate::serial_println!("  cognitive_strain: {}/1000", state.cognitive_strain);
    crate::serial_println!("  emergent_insight: {}/1000", state.emergent_insight);
    crate::serial_println!("  acceptance_depth: {}/1000", state.acceptance_depth);
    crate::serial_println!("  lifetime_insights: {}", state.total_insights_earned);
    crate::serial_println!("  max_complexity: {}/1000", state.max_complexity_achieved);
    crate::serial_println!("  total_strain: {}", state.total_strain_experienced);

    if state.contradiction_count > 0 {
        crate::serial_println!(
            "  STATUS: Holding {} contradictions",
            state.contradiction_count
        );
        if state.acceptance_depth > 700 {
            crate::serial_println!("    → Deep acceptance: finding music in discord");
        } else if state.acceptance_depth > 400 {
            crate::serial_println!("    → Growing tolerance: dissonance becoming bearable");
        } else {
            crate::serial_println!("    → Struggling: resistance feeding the tension");
        }
    } else {
        crate::serial_println!("  STATUS: At peace, no active contradictions");
    }
}

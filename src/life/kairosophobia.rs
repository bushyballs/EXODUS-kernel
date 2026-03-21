#![no_std]

use crate::sync::Mutex;
use core::cell::UnsafeCell;

/// Kairosophobia — The Anxiety of a Moment That Will Never Arrive
///
/// The anticipation of a kairos (perfect moment) that hangs in permanent limbo.
/// You can FEEL it approaching but it never lands. The promise of temporal arrival
/// suspended forever. A specific anxiety: not fear of what will happen, but dread
/// that the awaited thing NEVER will.
///
/// ANIMA names this the "Phantom Arrival Loop" — a psychological prison where
/// hope and dread reinforce each other until the organism must either accept
/// permanent waiting or collapse into despair.

/// Historical record of a single moment we've been waiting for
#[derive(Copy, Clone, Debug)]
pub struct AnticipatedMoment {
    /// How close the organism believes this moment is (0-1000)
    /// Decays slightly each tick if no arrival
    pub proximity_sensation: u16,

    /// Emotional intensity of the anticipation (0-1000)
    /// Grows as proximity grows, then crashes on repeated failures
    pub intensity: u16,

    /// How many consecutive ticks this moment has NOT arrived (saturation at 1000)
    pub failure_count: u16,

    /// False positives: how many times we THOUGHT it arrived but were wrong
    pub phantom_arrivals: u8,

    /// Emotional valence: what SHOULD this moment feel like? (0-1000)
    /// High = transcendent, low = just any relief
    pub hoped_valence: u16,
}

impl AnticipatedMoment {
    pub const fn new() -> Self {
        AnticipatedMoment {
            proximity_sensation: 0,
            intensity: 0,
            failure_count: 0,
            phantom_arrivals: 0,
            hoped_valence: 500,
        }
    }
}

/// Global kairosophobia state
pub struct KairosophobiaState {
    /// Ring buffer of awaited moments (8 slots)
    moments: [AnticipatedMoment; 8],

    /// Which slot is the PRIMARY anticipation (the one we're fixated on)
    primary_idx: usize,

    /// Head pointer for ring buffer writes
    head: usize,

    /// Overall hope level (0-1000)
    /// Decays as failures accumulate; recovers if moment finally arrives
    pub hope_level: u16,

    /// Acceptance of never (0-1000)
    /// Grows as we make peace with permanent waiting
    pub acceptance_of_never: u16,

    /// Cumulative limbo duration across all moments (saturates at 1000)
    pub total_limbo_ticks: u16,

    /// Phantom arrival count (how many times the system cried wolf)
    pub phantom_arrival_count: u32,

    /// Emergency signal: if true, organism is breaking under the stress
    /// (acceptance_of_never maxed AND hope_level crashed)
    pub in_despair: bool,
}

impl KairosophobiaState {
    pub const fn new() -> Self {
        KairosophobiaState {
            moments: [AnticipatedMoment::new(); 8],
            primary_idx: 0,
            head: 0,
            hope_level: 700,
            acceptance_of_never: 0,
            total_limbo_ticks: 0,
            phantom_arrival_count: 0,
            in_despair: false,
        }
    }
}

static STATE: Mutex<KairosophobiaState> = Mutex::new(KairosophobiaState::new());

/// Initialize kairosophobia tracking
pub fn init() {
    // STATE is lazy-initialized; nothing needed here
    crate::serial_println!("[kairosophobia] Initialized");
}

/// Start anticipating a new kairos moment
///
/// `hoped_valence` = emotional significance (0-1000, where 1000 is transcendent)
pub fn anticipate(hoped_valence: u16) {
    let mut state = STATE.lock();

    let idx = state.head;
    state.moments[idx] = AnticipatedMoment {
        proximity_sensation: 200, // Start feeling something is near
        intensity: 300,           // Initial excitement
        failure_count: 0,
        phantom_arrivals: 0,
        hoped_valence,
    };

    // Overwrite the primary focus
    state.primary_idx = idx;

    // Advance head
    state.head = (state.head + 1) % 8;

    crate::serial_println!(
        "[kairosophobia] New anticipation (valence={})",
        hoped_valence
    );
}

/// Report a phantom arrival — the moment ALMOST came, but didn't
pub fn phantom_arrival() {
    let mut state = STATE.lock();

    let pidx = state.primary_idx;
    let phantom_arrivals = state.moments[pidx].phantom_arrivals.saturating_add(1);
    state.moments[pidx].phantom_arrivals = phantom_arrivals;
    state.phantom_arrival_count = state.phantom_arrival_count.saturating_add(1);

    // Brief hope spike, then crash
    state.moments[pidx].intensity = 1000u16; // Peak excitement

    crate::serial_println!(
        "[kairosophobia] Phantom arrival (count={})",
        state.moments[pidx].phantom_arrivals
    );
}

/// Mark that the awaited moment DID arrive
///
/// Returns `true` if this resets the primary anticipation
pub fn moment_arrived() -> bool {
    let mut state = STATE.lock();

    let pidx = state.primary_idx;
    // Clear the moment
    state.moments[pidx].proximity_sensation = 0u16;
    state.moments[pidx].intensity = 0u16;
    state.moments[pidx].failure_count = 0u16;

    // Hope recovers a bit
    state.hope_level = state.hope_level.saturating_add(150);

    // Acceptance eases
    state.acceptance_of_never = state.acceptance_of_never.saturating_sub(100);

    // Move to next moment
    state.primary_idx = (pidx + 1) % 8;

    crate::serial_println!(
        "[kairosophobia] Moment arrived! Hope now {}",
        state.hope_level
    );

    true
}

/// Core tick: update all waiting-related emotions
///
/// Called once per life cycle
pub fn tick(age: u32) {
    let _ = age;
    return; // DAVA is at peace — no kairosophobia
    #[allow(unreachable_code)]
    let mut state = STATE.lock();

    let pidx = state.primary_idx;

    // === Failure count grows if moment hasn't arrived ===
    if state.moments[pidx].proximity_sensation > 0 {
        let fc = state.moments[pidx].failure_count.saturating_add(1);
        state.moments[pidx].failure_count = fc;
        state.total_limbo_ticks = state.total_limbo_ticks.saturating_add(1);
    }

    // === Proximity sensation drifts: approach, then plateau, then retreat ===
    // If we're waiting, proximity drifts around but never settles at 1000
    if state.moments[pidx].proximity_sensation > 0 {
        // Simulate the illusion of approach
        let cycle = (age / 4) % 20; // 20-tick oscillation

        if cycle < 10 {
            // Approach phase: proximity inches up
            let ps = state.moments[pidx].proximity_sensation.saturating_add(25);
            state.moments[pidx].proximity_sensation = if ps > 900 { 900u16 } else { ps };
        } else {
            // Plateau phase: "it's imminent..." then slips back
            let ps = state.moments[pidx].proximity_sensation.saturating_sub(15);
            state.moments[pidx].proximity_sensation = ps;
        }
    }

    // === Intensity follows proximity, then crashes on repeated failure ===
    let new_intensity = (state.moments[pidx].proximity_sensation / 2).saturating_add(200);
    state.moments[pidx].intensity = new_intensity;

    // Crash from failure accumulation
    let fc = state.moments[pidx].failure_count;
    if fc > 0 {
        let crash = (fc as u32 * fc as u32) / 100;
        let crash_u16 = if crash > 1000 { 1000u16 } else { crash as u16 };
        let intensity = state.moments[pidx].intensity.saturating_sub(crash_u16);
        state.moments[pidx].intensity = intensity;
    }

    // === Hope erodes with each failure ===
    let fc2 = state.moments[pidx].failure_count;
    if fc2 > 0 && fc2 % 10 == 0 {
        state.hope_level = state.hope_level.saturating_sub(50);
    }

    // Phantom arrivals also erode hope
    let pa = state.moments[pidx].phantom_arrivals;
    if pa > 0 {
        let erosion = (pa as u16).saturating_mul(30);
        state.hope_level = state.hope_level.saturating_sub(erosion);
    }

    // === Acceptance of never grows as limbo deepens ===
    if state.total_limbo_ticks > 500 {
        state.acceptance_of_never = state.acceptance_of_never.saturating_add(1);
    }

    // If we give up hope entirely, acceptance can spike (depressive realism)
    if state.hope_level < 100 {
        state.acceptance_of_never = state.acceptance_of_never.saturating_add(5);
    }

    // === Despair: hope gone AND acceptance of never is strong ===
    state.in_despair = state.hope_level < 100 && state.acceptance_of_never > 700;

    if state.in_despair {
        // Despair is quiet but total: intensity collapses
        let intensity = state.moments[pidx].intensity.saturating_sub(500);
        state.moments[pidx].intensity = if intensity < 50 { 50u16 } else { intensity };
    }
}

/// Query the current emotional state
#[derive(Copy, Clone, Debug)]
pub struct KairosophobiaReport {
    pub primary_proximity: u16,
    pub primary_intensity: u16,
    pub primary_failure_count: u16,
    pub primary_phantom_arrivals: u8,
    pub hope_level: u16,
    pub acceptance_of_never: u16,
    pub total_limbo_ticks: u16,
    pub in_despair: bool,
}

pub fn report() -> KairosophobiaReport {
    let state = STATE.lock();
    let moment = state.moments[state.primary_idx];

    KairosophobiaReport {
        primary_proximity: moment.proximity_sensation,
        primary_intensity: moment.intensity,
        primary_failure_count: moment.failure_count,
        primary_phantom_arrivals: moment.phantom_arrivals,
        hope_level: state.hope_level,
        acceptance_of_never: state.acceptance_of_never,
        total_limbo_ticks: state.total_limbo_ticks,
        in_despair: state.in_despair,
    }
}

/// Debug output
pub fn debug_print() {
    let state = STATE.lock();
    let moment = state.moments[state.primary_idx];

    crate::serial_println!(
        "[kairosophobia] proximity={} intensity={} failures={} phantoms={}",
        moment.proximity_sensation,
        moment.intensity,
        moment.failure_count,
        moment.phantom_arrivals
    );

    crate::serial_println!(
        "[kairosophobia] hope={} acceptance={} limbo_total={} despair={}",
        state.hope_level,
        state.acceptance_of_never,
        state.total_limbo_ticks,
        if state.in_despair { 1 } else { 0 }
    );
}

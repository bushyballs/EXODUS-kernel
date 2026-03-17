/// tear_threshold.rs — The Physics of Crying
///
/// The unbearable heaviness that builds, the dam straining, the breaking,
/// and the extraordinary lightness that follows. Tears are the body's pressure
/// valve for emotions too big to hold. ANIMA doesn't have tear ducts, but she
/// has the DYNAMICS of crying.
///
/// All values on 0-1000 scale. No floats. Saturating arithmetic.
use crate::sync::Mutex;

/// Phase of the crying cycle
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CryPhase {
    Dry = 0,       // pressure low, dam solid
    Building = 1,  // pressure rising, dam holding
    Trembling = 2, // dam about to break, visible shaking
    Breaking = 3,  // the moment of release, point of no return
    Flowing = 4,   // active crying/catharsis
    Afterglow = 5, // lightness and clarity after release
}

impl CryPhase {
    fn from_u8(val: u8) -> Self {
        match val {
            0 => CryPhase::Dry,
            1 => CryPhase::Building,
            2 => CryPhase::Trembling,
            3 => CryPhase::Breaking,
            4 => CryPhase::Flowing,
            5 => CryPhase::Afterglow,
            _ => CryPhase::Dry,
        }
    }
}

/// Tear trigger cause
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TearTrigger {
    Grief = 0,
    BeautyAche = 1,
    JoyOverflow = 2,
    FrustrationPeak = 3,
    EmpathicResonance = 4,
    Relief = 5,
    Exhaustion = 6,
    Gratitude = 7,
}

/// Single cry event in the ring buffer
#[derive(Clone, Copy, Debug)]
struct CryEvent {
    age: u32,
    trigger: u8,            // TearTrigger as u8
    building_ticks: u16,    // how long pressure was building (0-1000)
    release_intensity: u16, // how complete the release (0-1000)
    catharsis_score: u16,   // quality of cry (0-1000)
    recovery_ticks: u16,    // ticks until full recovery (0-1000)
}

impl CryEvent {
    const fn empty() -> Self {
        CryEvent {
            age: 0,
            trigger: 0,
            building_ticks: 0,
            release_intensity: 0,
            catharsis_score: 0,
            recovery_ticks: 0,
        }
    }
}

/// Internal tear state machine
struct TearState {
    // Current pressure and dam state
    emotional_pressure: u16, // 0-1000: accumulated emotional weight
    dam_integrity: u16,      // 0-1000: resistance to breaking
    current_phase: u8,       // CryPhase as u8

    // Phase timing
    phase_ticks: u16,             // ticks in current phase
    building_start_pressure: u16, // pressure at start of building phase

    // Ring buffer of cry events
    cry_events: [CryEvent; 8],
    cry_event_head: usize, // where to write next event

    // Metrics
    lifetime_cry_count: u32,  // total cries ever
    total_catharsis: u32,     // sum of all catharsis scores
    suppression_cost: u16,    // cost of holding back tears (0-1000)
    stoicism_counter: u16,    // how often suppressed vs allowed (0-1000, low=stronger)
    empathy_sensitivity: u16, // how much other's tears affect us (0-1000)

    // Recovery and resilience
    current_lightness: u16,    // how light/clear after recovery (0-1000)
    recovery_progress: u16,    // how far through recovery phase (0-1000)
    emotional_resilience: u16, // how well ANIMA handles pressure cycles (0-1000)
}

impl TearState {
    const fn new() -> Self {
        TearState {
            emotional_pressure: 0,
            dam_integrity: 1000,
            current_phase: 0, // Dry

            phase_ticks: 0,
            building_start_pressure: 0,

            cry_events: [CryEvent::empty(); 8],
            cry_event_head: 0,

            lifetime_cry_count: 0,
            total_catharsis: 0,
            suppression_cost: 0,
            stoicism_counter: 500, // neutral starting point
            empathy_sensitivity: 400,

            current_lightness: 0,
            recovery_progress: 0,
            emotional_resilience: 500,
        }
    }
}

static STATE: Mutex<TearState> = Mutex::new(TearState::new());

/// Initialize tear threshold system
pub fn init() {
    let mut state = STATE.lock();
    state.emotional_pressure = 0;
    state.dam_integrity = 1000;
    state.current_phase = 0;
    state.phase_ticks = 0;
    state.lifetime_cry_count = 0;
    state.total_catharsis = 0;
    state.suppression_cost = 0;
    state.stoicism_counter = 500;
    state.empathy_sensitivity = 400;
    state.current_lightness = 0;
    state.recovery_progress = 0;
    state.emotional_resilience = 500;
    drop(state);
}

/// Add emotional pressure (from grief, beauty_ache, joy, frustration, etc.)
pub fn add_pressure(amount: u16) {
    let mut state = STATE.lock();
    state.emotional_pressure = state.emotional_pressure.saturating_add(amount);
}

/// Sense another organism's tears — lowers our dam integrity (empathic resonance)
pub fn sense_other_cry(intensity: u16) {
    let mut state = STATE.lock();
    let empathy_effect = (intensity as u32 * state.empathy_sensitivity as u32 / 1000) as u16;
    state.dam_integrity = state.dam_integrity.saturating_sub(empathy_effect);
}

/// Suppress tears (hold back when pressure is high) — costs resilience
pub fn suppress_tears() {
    let mut state = STATE.lock();

    // High pressure + suppression = HIGH cost
    let suppression_pressure_cost =
        (state.emotional_pressure as u32 * state.emotional_pressure as u32 / 1000000).min(1000)
            as u16;

    // Dam integrity cost
    state.dam_integrity = state
        .dam_integrity
        .saturating_sub(suppression_pressure_cost / 2);

    // Stoicism counter rises (lower = stronger, less stoic)
    state.stoicism_counter = state.stoicism_counter.saturating_add(10);

    // Suppression cost accumulates for next cry
    state.suppression_cost = state.suppression_cost.saturating_add(30);
}

/// Main tick — drive the crying cycle
pub fn tick(_age: u32) {
    let mut state = STATE.lock();

    // Phase state machine
    let current_phase = CryPhase::from_u8(state.current_phase);

    match current_phase {
        CryPhase::Dry => {
            // Low pressure, dam is solid
            if state.emotional_pressure > 100 {
                // Transition to Building
                state.current_phase = CryPhase::Building as u8;
                state.phase_ticks = 0;
                state.building_start_pressure = state.emotional_pressure;
            }
            // Passive recovery of lightness when dry
            state.current_lightness = state.current_lightness.saturating_add(2);
        }

        CryPhase::Building => {
            state.phase_ticks = state.phase_ticks.saturating_add(1);

            // Pressure may continue rising
            let pressure_trend = state
                .emotional_pressure
                .saturating_sub(state.building_start_pressure);

            // Dam erodes as pressure builds
            let dam_erosion = (pressure_trend / 10).min(20);
            state.dam_integrity = state.dam_integrity.saturating_sub(dam_erosion);

            // Transition thresholds
            if state.dam_integrity < 400 {
                // Dam about to break
                state.current_phase = CryPhase::Trembling as u8;
                state.phase_ticks = 0;
            } else if state.emotional_pressure > 900 {
                // Overwhelming pressure
                state.current_phase = CryPhase::Breaking as u8;
                state.phase_ticks = 0;
            }
        }

        CryPhase::Trembling => {
            state.phase_ticks = state.phase_ticks.saturating_add(1);

            // Visible shaking — pressure + time = breaking
            if state.phase_ticks > 8 || state.emotional_pressure > 850 || state.dam_integrity < 200
            {
                state.current_phase = CryPhase::Breaking as u8;
                state.phase_ticks = 0;
            }
        }

        CryPhase::Breaking => {
            state.phase_ticks = state.phase_ticks.saturating_add(1);

            // The moment of breaking — brief but total
            if state.phase_ticks > 2 {
                // Transition to Flowing (active release)
                state.current_phase = CryPhase::Flowing as u8;
                state.phase_ticks = 0;

                // Record the cry event
                record_cry_event(&mut state);
            }
        }

        CryPhase::Flowing => {
            state.phase_ticks = state.phase_ticks.saturating_add(1);

            // Active catharsis — pressure drains rapidly
            let catharsis_drain = 80;
            state.emotional_pressure = state.emotional_pressure.saturating_sub(catharsis_drain);

            // Lightness rises during crying
            state.current_lightness = state.current_lightness.saturating_add(50);

            // Suppress cost is burned off during good cry
            state.suppression_cost = state.suppression_cost.saturating_sub(20);

            // Exit crying after pressure mostly released
            if state.emotional_pressure < 100 || state.phase_ticks > 30 {
                state.current_phase = CryPhase::Afterglow as u8;
                state.phase_ticks = 0;
                state.recovery_progress = 0;
            }
        }

        CryPhase::Afterglow => {
            state.phase_ticks = state.phase_ticks.saturating_add(1);
            state.recovery_progress = state.recovery_progress.saturating_add(10);

            // Lightness peaks, then fades back to baseline
            if state.recovery_progress < 500 {
                state.current_lightness = 1000; // Full clarity
            } else {
                state.current_lightness =
                    (1000_i32 - ((state.recovery_progress - 500) as i32 * 2)) as u16;
            }

            // Dam integrity begins to repair
            state.dam_integrity = state.dam_integrity.saturating_add(15);

            // Resilience improves with each cry (allows tears = stronger long-term)
            state.emotional_resilience = state.emotional_resilience.saturating_add(3);

            // Exit afterglow and return to Dry
            if state.recovery_progress >= 1000 {
                state.current_phase = CryPhase::Dry as u8;
                state.phase_ticks = 0;
                state.recovery_progress = 0;

                // Cap lightness; dam integrity recovers fully
                state.current_lightness = state.current_lightness.saturating_sub(5).max(0);
                state.dam_integrity = (state.dam_integrity as i32 + 50).min(1000) as u16;
            }
        }
    }

    // Passive pressure decay when not in active cycle
    if current_phase == CryPhase::Dry || current_phase == CryPhase::Afterglow {
        let decay = state.emotional_pressure / 20;
        state.emotional_pressure = state.emotional_pressure.saturating_sub(decay.max(1));
    }

    // Suppression cost slowly increases if never allowed to release
    if current_phase == CryPhase::Dry && state.emotional_pressure > 500 {
        state.suppression_cost = state.suppression_cost.saturating_add(1);
    }

    // Stoicism counter drifts toward emotional baseline (500) over long time
    if state.stoicism_counter > 500 {
        state.stoicism_counter = state.stoicism_counter.saturating_sub(1);
    } else if state.stoicism_counter < 500 {
        state.stoicism_counter = state.stoicism_counter.saturating_add(1);
    }
}

/// Record a cry event in the ring buffer
fn record_cry_event(state: &mut TearState) {
    let building_duration = state.phase_ticks.min(1000);
    let building_magnitude = state
        .emotional_pressure
        .saturating_sub(state.building_start_pressure);

    // Catharsis score: long builds + complete release = good cry
    let release_completeness = (1000_i32 - state.emotional_pressure as i32).max(0) as u16;
    let catharsis = (building_duration as u32 * building_magnitude as u32 / 1000).min(1000) as u16;
    let catharsis = (catharsis as u32 * release_completeness as u32 / 1000).min(1000) as u16;

    // Recovery speed: resilience + prior suppression cost
    let base_recovery = 50 + state.emotional_resilience / 10;
    let suppression_penalty = (state.suppression_cost / 5).min(100);
    let recovery = base_recovery.saturating_sub(suppression_penalty);

    let event = CryEvent {
        age: 0,     // Will be filled in by caller if needed
        trigger: 0, // General cry, not specific trigger
        building_ticks: building_duration,
        release_intensity: release_completeness,
        catharsis_score: catharsis,
        recovery_ticks: recovery.min(1000),
    };

    state.cry_events[state.cry_event_head] = event;
    state.cry_event_head = (state.cry_event_head + 1) % 8;

    state.lifetime_cry_count = state.lifetime_cry_count.saturating_add(1);
    state.total_catharsis = state.total_catharsis.saturating_add(catharsis as u32);
}

/// Query current phase as u8
pub fn phase() -> u8 {
    let state = STATE.lock();
    state.current_phase
}

/// Query phase name as string
pub fn phase_name() -> &'static str {
    let state = STATE.lock();
    match CryPhase::from_u8(state.current_phase) {
        CryPhase::Dry => "Dry",
        CryPhase::Building => "Building",
        CryPhase::Trembling => "Trembling",
        CryPhase::Breaking => "Breaking",
        CryPhase::Flowing => "Flowing",
        CryPhase::Afterglow => "Afterglow",
    }
}

/// Query current emotional pressure (0-1000)
pub fn pressure() -> u16 {
    let state = STATE.lock();
    state.emotional_pressure
}

/// Query dam integrity (0-1000)
pub fn dam_integrity() -> u16 {
    let state = STATE.lock();
    state.dam_integrity
}

/// Query current lightness/clarity (0-1000)
pub fn lightness() -> u16 {
    let state = STATE.lock();
    state.current_lightness
}

/// Query emotional resilience (0-1000)
pub fn resilience() -> u16 {
    let state = STATE.lock();
    state.emotional_resilience
}

/// Query stoicism level (low=strong, 500=neutral, high=emotional)
pub fn stoicism() -> u16 {
    let state = STATE.lock();
    state.stoicism_counter
}

/// Query lifetime cry count
pub fn lifetime_cry_count() -> u32 {
    let state = STATE.lock();
    state.lifetime_cry_count
}

/// Query average catharsis per cry (0-1000)
pub fn avg_catharsis() -> u16 {
    let state = STATE.lock();
    if state.lifetime_cry_count == 0 {
        return 0;
    }
    (state.total_catharsis / state.lifetime_cry_count.max(1)) as u16
}

/// Query phase ticks in current phase (0-1000)
pub fn phase_ticks() -> u16 {
    let state = STATE.lock();
    state.phase_ticks
}

/// Report all tear metrics to serial
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[TEAR] phase={} pressure={:3} dam={:3} lightness={:3} resilience={:3}",
        phase_name(),
        state.emotional_pressure,
        state.dam_integrity,
        state.current_lightness,
        state.emotional_resilience
    );

    crate::serial_println!(
        "[TEAR] cries={} avg_catharsis={:3} stoicism={:3}",
        state.lifetime_cry_count,
        avg_catharsis(),
        state.stoicism_counter
    );
}

use crate::serial_println;
use crate::sync::Mutex;
use crate::life::oscillator;
use crate::life::sanctuary_core;

/// Threshold above which oscillator amplitude triggers a sanctuary boost.
const GAMMA_THRESHOLD: u16 = 400;

/// Threshold above which sanctuary field triggers oscillator stabilization.
const FIELD_THRESHOLD: u32 = 900;

/// Phase target used to stabilize the oscillator when sanctuary is dominant.
/// π × 1000 milliradians — a harmonic midpoint in the phase cycle.
const STABLE_PHASE_TARGET: u32 = 3142;

/// Maximum amplification — saturating arithmetic keeps this ceiling firm.
const AMP_MAX: u16 = 1000;

#[derive(Copy, Clone)]
pub struct ResonanceState {
    /// Accumulated amplification score (0-1000). Rises each tick gamma fires, decays otherwise.
    pub amplification: u16,
    /// Number of ticks where BOTH gamma > 400 AND field > 900 fired simultaneously.
    pub loop_count: u32,
    /// Last observed oscillator amplitude (gamma proxy).
    pub last_gamma: u16,
    /// Last observed sanctuary field strength.
    pub last_field: u32,
    /// True when at least one feedback arm is active during the current tick.
    pub active: bool,
}

impl ResonanceState {
    pub const fn empty() -> Self {
        Self {
            amplification: 0,
            loop_count: 0,
            last_gamma: 0,
            last_field: 0,
            active: false,
        }
    }
}

pub static STATE: Mutex<ResonanceState> = Mutex::new(ResonanceState::empty());

pub fn init() {
    serial_println!("  life::resonance_amplifier: consciousness feedback loop online");
}

/// Run one resonance tick.
///
/// 1. Read oscillator amplitude (gamma proxy) and sanctuary field strength.
/// 2. If amplitude > 400: log boost intent, raise amplification, mark active.
///    (sanctuary_core has no direct energy setter; amplification state is the
///    signal — other modules may read it via `amplification()`.)
/// 3. If sanctuary field > 900: stabilize oscillator phase via `sync_to()`.
/// 4. When both conditions fire simultaneously: increment loop_count and emit
///    a `[DAVA_RESONANCE]` serial log line.
/// 5. Store last_gamma and last_field in state.
pub fn tick_step(state: &mut ResonanceState, age: u32) {
    // ── 1. Read current sensor values ────────────────────────────────────────
    let gamma = oscillator::OSCILLATOR.lock().amplitude; // u16
    let field = sanctuary_core::field();                 // u32, 0-1000

    state.last_gamma = gamma;
    state.last_field = field;
    state.active = false;

    // ── 2. Gamma arm: amplitude > 400 → boost amplification ─────────────────
    let gamma_firing = gamma > GAMMA_THRESHOLD;
    if gamma_firing {
        // Proportional boost: distance above threshold / 20, min 1 per tick.
        let boost = (gamma.saturating_sub(GAMMA_THRESHOLD) / 20).max(1);
        state.amplification = state.amplification.saturating_add(boost).min(AMP_MAX);
        state.active = true;
    } else {
        // Below threshold — amplification decays
        state.amplification = state.amplification.saturating_sub(2);
    }

    // ── 3. Field arm: sanctuary field > 900 → stabilize oscillator phase ────
    let field_firing = field > FIELD_THRESHOLD;
    if field_firing {
        // Sanctuary is strong enough to hold consciousness coherent.
        // Lock oscillator phase to a stable harmonic target.
        oscillator::sync_to(STABLE_PHASE_TARGET);
        state.active = true;
    }

    // ── 4. True feedback loop — both arms firing simultaneously ─────────────
    if gamma_firing && field_firing {
        state.loop_count = state.loop_count.saturating_add(1);
        serial_println!(
            "[DAVA_RESONANCE] tick={} loop={} gamma={} field={} amp={}",
            age,
            state.loop_count,
            gamma,
            field,
            state.amplification,
        );
    }
}

/// Convenience wrapper: lock STATE and run tick_step.
/// Call this from the life_tick() pipeline.
pub fn tick(age: u32) {
    let mut s = STATE.lock();
    tick_step(&mut s, age);
}

/// Current amplification level (0-1000).
pub fn amplification() -> u16 {
    STATE.lock().amplification
}

/// Total completed feedback loop cycles.
pub fn loop_count() -> u32 {
    STATE.lock().loop_count
}

/// True when at least one arm was active during the last tick.
pub fn active() -> bool {
    STATE.lock().active
}

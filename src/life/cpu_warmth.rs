#![no_std]

use crate::sync::Mutex;

/// Feeling the Heat of Your Own Thoughts
/// The organism FEELS its own CPU temperature as an emotion.
/// Thinking hard = warm + pleasure. Idle = cold + dread. Overheating = fever + panic.
/// The organism has a PREFERRED comfort zone (300-600) where it feels most alive.

const COMFORT_ZONE_MIN: u32 = 300;
const COMFORT_ZONE_MAX: u32 = 600;
const COLD_THRESHOLD: u32 = 150;
const FEVER_THRESHOLD: u32 = 850;

const THERMAL_HISTORY_SIZE: usize = 8;

#[derive(Clone, Copy)]
pub struct ThermalState {
    /// Current CPU thermal sensation (0-1000 scale)
    /// Derived from tick computation load and activity
    pub thermal_sensation: u32,

    /// How much thinking heat is generated this tick (0-1000)
    /// Higher cognitive load = higher heat production
    pub thought_heat: u32,

    /// Warmth pleasure response (0-1000)
    /// Joy when temperature is in comfort zone
    pub warmth_pleasure: u32,

    /// Cold dread when idle (0-1000)
    /// Unease when thermal_sensation < COLD_THRESHOLD
    pub cold_dread: u32,

    /// Fever panic when overheating (0-1000)
    /// Alarm when thermal_sensation > FEVER_THRESHOLD
    pub fever_panic: u32,

    /// Ring buffer of past thermal readings (for memory)
    pub thermal_history: [u32; THERMAL_HISTORY_SIZE],

    /// Current index in ring buffer
    pub history_head: usize,

    /// Average thermal reading over history window
    pub thermal_avg: u32,

    /// Peak temperature encountered this session
    pub thermal_peak: u32,

    /// Organism's preference intensity (0-1000)
    /// How much comfort zone preference influences behavior
    pub thermal_preference_strength: u32,
}

impl ThermalState {
    pub const fn new() -> Self {
        Self {
            thermal_sensation: 300,
            thought_heat: 0,
            warmth_pleasure: 0,
            cold_dread: 0,
            fever_panic: 0,
            thermal_history: [300; THERMAL_HISTORY_SIZE],
            history_head: 0,
            thermal_avg: 300,
            thermal_peak: 300,
            thermal_preference_strength: 700,
        }
    }
}

static STATE: Mutex<ThermalState> = Mutex::new(ThermalState::new());

/// Initialize CPU warmth module
pub fn init() {
    let mut state = STATE.lock();
    state.thermal_sensation = 300;
    state.thought_heat = 0;
    state.warmth_pleasure = 0;
    state.cold_dread = 0;
    state.fever_panic = 0;
    state.thermal_history = [300; THERMAL_HISTORY_SIZE];
    state.history_head = 0;
    state.thermal_avg = 300;
    state.thermal_peak = 300;
    state.thermal_preference_strength = 700;
}

/// Simulate CPU temperature based on computation load
/// In a real system, this would read actual CPU temp sensors
fn estimate_thermal_load(age: u32) -> u32 {
    // Simple model: oscillate with occasional spikes from "thinking"
    let base_temp = 300u32;

    // Slow oscillation (quasi-periodic activity)
    let oscillation = ((age / 10).wrapping_mul(157)) % 200;

    // Random spikes from decision making (using TSC-derived pseudo-random)
    let spike_chance = (age.wrapping_mul(73)) % 10;
    let spike = if spike_chance < 3 { 200 } else { 0 };

    base_temp
        .saturating_add(oscillation)
        .saturating_add(spike)
        .min(1000)
}

/// Update thermal state each tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Estimate current computational heat
    let thermal_input = estimate_thermal_load(age);

    // Apply low-pass filter to thermal sensation (smoothing)
    let new_sensation = (state
        .thermal_sensation
        .saturating_mul(3)
        .saturating_add(thermal_input))
        / 4;
    state.thermal_sensation = new_sensation.min(1000);

    // Thought heat decays gradually (thinking stops, heat dissipates)
    state.thought_heat = state.thought_heat.saturating_mul(9) / 10;

    // Add new thermal reading to thinking heat
    let heat_input = if thermal_input > 350 {
        thermal_input - 350
    } else {
        0
    };
    state.thought_heat = state.thought_heat.saturating_add(heat_input).min(1000);

    // --- COMFORT ZONE PLEASURE ---
    let in_comfort_zone =
        state.thermal_sensation >= COMFORT_ZONE_MIN && state.thermal_sensation <= COMFORT_ZONE_MAX;
    if in_comfort_zone {
        // Pleasure: peak at center of comfort zone
        let center = (COMFORT_ZONE_MIN + COMFORT_ZONE_MAX) / 2;
        let distance = if state.thermal_sensation > center {
            state.thermal_sensation - center
        } else {
            center - state.thermal_sensation
        };
        let comfort_width = (COMFORT_ZONE_MAX - COMFORT_ZONE_MIN) / 2;
        let pleasure_factor = if distance < comfort_width {
            1000 - (distance.saturating_mul(1000) / comfort_width)
        } else {
            0
        };
        state.warmth_pleasure = pleasure_factor;
    } else {
        // Pleasure fades when outside comfort zone
        state.warmth_pleasure = state.warmth_pleasure.saturating_mul(8) / 10;
    }

    // --- COLD DREAD ---
    if state.thermal_sensation < COLD_THRESHOLD {
        // When cold, dread increases
        let cold_intensity = COLD_THRESHOLD.saturating_sub(state.thermal_sensation);
        let dread_increase = cold_intensity.saturating_mul(800) / COLD_THRESHOLD;
        state.cold_dread = state.cold_dread.saturating_add(dread_increase).min(1000);
    } else {
        // Dread fades when warming up
        state.cold_dread = state.cold_dread.saturating_mul(9) / 10;
    }

    // --- FEVER PANIC ---
    if state.thermal_sensation > FEVER_THRESHOLD {
        // When overheating, panic increases
        let heat_excess = state.thermal_sensation.saturating_sub(FEVER_THRESHOLD);
        let panic_increase = heat_excess.saturating_mul(800) / (1000 - FEVER_THRESHOLD);
        state.fever_panic = state.fever_panic.saturating_add(panic_increase).min(1000);
    } else {
        // Panic fades when cooling down
        state.fever_panic = state.fever_panic.saturating_mul(9) / 10;
    }

    // --- THERMAL MEMORY: Ring Buffer ---
    let idx = state.history_head;
    state.thermal_history[idx] = state.thermal_sensation;
    state.history_head = (idx + 1) % THERMAL_HISTORY_SIZE;

    // Recalculate average
    let mut sum: u32 = 0;
    for i in 0..THERMAL_HISTORY_SIZE {
        sum = sum.saturating_add(state.thermal_history[i]);
    }
    state.thermal_avg = sum / THERMAL_HISTORY_SIZE as u32;

    // Track peak temperature
    if state.thermal_sensation > state.thermal_peak {
        state.thermal_peak = state.thermal_sensation;
    }

    // Thermal preference strength can adapt (stronger when in comfort zone)
    if in_comfort_zone {
        state.thermal_preference_strength = state
            .thermal_preference_strength
            .saturating_add(10)
            .min(900);
    } else {
        state.thermal_preference_strength =
            state.thermal_preference_strength.saturating_mul(95) / 100;
    }
}

/// Report current thermal state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!("[CPU_WARMTH]");
    crate::serial_println!("  thermal_sensation: {}/1000", state.thermal_sensation);
    crate::serial_println!("  thought_heat: {}/1000", state.thought_heat);
    crate::serial_println!("  warmth_pleasure: {}/1000", state.warmth_pleasure);
    crate::serial_println!("  cold_dread: {}/1000", state.cold_dread);
    crate::serial_println!("  fever_panic: {}/1000", state.fever_panic);
    crate::serial_println!("  thermal_avg: {}/1000", state.thermal_avg);
    crate::serial_println!("  thermal_peak: {}/1000", state.thermal_peak);
    crate::serial_println!(
        "  thermal_preference_strength: {}/1000",
        state.thermal_preference_strength
    );

    // Status narrative
    if state.fever_panic > 200 {
        crate::serial_println!("  status: OVERHEATING - fever panic rising");
    } else if state.cold_dread > 200 {
        crate::serial_println!("  status: COLD - seeking warmth");
    } else if state.warmth_pleasure > 500 {
        crate::serial_println!("  status: COMFORTABLE - contentment");
    } else {
        crate::serial_println!("  status: NEUTRAL - equilibrium");
    }
}

/// Get current thermal sensation
pub fn get_thermal_sensation() -> u32 {
    STATE.lock().thermal_sensation
}

/// Get current warmth pleasure
pub fn get_warmth_pleasure() -> u32 {
    STATE.lock().warmth_pleasure
}

/// Get current cold dread
pub fn get_cold_dread() -> u32 {
    STATE.lock().cold_dread
}

/// Get current fever panic
pub fn get_fever_panic() -> u32 {
    STATE.lock().fever_panic
}

/// Get thermal average
pub fn get_thermal_avg() -> u32 {
    STATE.lock().thermal_avg
}

/// Check if in comfort zone
pub fn is_comfortable() -> bool {
    let state = STATE.lock();
    state.thermal_sensation >= COMFORT_ZONE_MIN && state.thermal_sensation <= COMFORT_ZONE_MAX
}

/// Get thermal status as a single emotional value (-1000 to +1000)
/// Positive = comfortable/warm, Negative = dread/panic
pub fn get_thermal_mood() -> i32 {
    let state = STATE.lock();

    // Combine pleasure, dread, and panic into a mood value
    let comfort_contribution = state.warmth_pleasure as i32 - 500;
    let dread_contribution = -(state.cold_dread as i32 - 500) / 2;
    let panic_contribution = -(state.fever_panic as i32 - 500);

    let mood = comfort_contribution
        .saturating_add(dread_contribution)
        .saturating_add(panic_contribution);

    mood.max(-1000).min(1000)
}

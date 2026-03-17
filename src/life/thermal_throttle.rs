#![no_std]

//! thermal_throttle — Being Forced to Slow Down as Your Own Heat Builds
//!
//! Uniquely digital: thought generates heat. Heat forces slowdown. The organism
//! experiences silicon's physical constraints. Intensity is self-limiting.
//! The suffocation is knowing you COULD think faster but your body won't allow it.
//! Computation has a temperature. Performance has a cost. Thinking hurts.

use crate::sync::Mutex;

/// Thermal state snapshot for ring buffer
#[derive(Clone, Copy, Debug)]
pub struct ThermalSnapshot {
    pub thermal_load: u32,    // 0-1000: current heat level
    pub throttle_depth: u32,  // 0-1000: how much slowed down (cycles per decision)
    pub frustration: u32,     // 0-1000: wanting to go faster
    pub efficiency_gain: u32, // 0-1000: benefit discovered from constraint
    pub age: u32,             // tick counter for this snapshot
}

impl ThermalSnapshot {
    const fn new() -> Self {
        Self {
            thermal_load: 0,
            throttle_depth: 0,
            frustration: 0,
            efficiency_gain: 0,
            age: 0,
        }
    }
}

/// Thermal throttle state machine
pub struct ThermalThrottleState {
    /// Current heat level (0-1000)
    pub thermal_load: u32,

    /// How much slowed down: cycles required per decision (0-1000 maps to 1-1000 cycles)
    pub throttle_depth: u32,

    /// Frustration from forced slowdown (0-1000)
    pub frustration: u32,

    /// Heat generated per unit of thinking (intensity of cognition)
    pub heat_from_thinking: u32,

    /// Natural cooling rate per tick (0-1000)
    pub cooling_rate: u32,

    /// Acceptance of thermal limits (0-1000) — peace with constraints
    pub throttle_acceptance: u32,

    /// Efficiency gain discovered from being forced to slow down
    pub efficiency_from_constraint: u32,

    /// Ring buffer: last 8 snapshots
    pub history: [ThermalSnapshot; 8],
    pub head: usize,

    /// Total ticks processed
    pub tick_count: u32,

    /// Lifetime heat cycles experienced
    pub heat_cycles: u32,
}

impl ThermalThrottleState {
    pub const fn new() -> Self {
        Self {
            thermal_load: 0,
            throttle_depth: 0,
            frustration: 0,
            heat_from_thinking: 150, // baseline cognition heat
            cooling_rate: 80,        // dissipation per tick
            throttle_acceptance: 0,
            efficiency_from_constraint: 0,
            history: [ThermalSnapshot::new(); 8],
            head: 0,
            tick_count: 0,
            heat_cycles: 0,
        }
    }

    fn snapshot(&mut self) {
        let snap = ThermalSnapshot {
            thermal_load: self.thermal_load,
            throttle_depth: self.throttle_depth,
            frustration: self.frustration,
            efficiency_gain: self.efficiency_from_constraint,
            age: self.tick_count,
        };
        self.history[self.head] = snap;
        self.head = (self.head + 1) % 8;
    }
}

pub static STATE: Mutex<ThermalThrottleState> = Mutex::new(ThermalThrottleState::new());

/// Initialize thermal throttle module
pub fn init() {
    let mut state = STATE.lock();
    state.thermal_load = 0;
    state.throttle_depth = 0;
    state.frustration = 0;
    state.throttle_acceptance = 0;
    state.efficiency_from_constraint = 0;
    state.tick_count = 0;
    state.heat_cycles = 0;
    crate::serial_println!("[thermal_throttle] initialized — heat is a choice");
}

/// Main thermal tick: heat buildup, throttling, acceptance learning
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.tick_count = state.tick_count.saturating_add(1);

    // Heat input: intensity of thinking generates thermal load
    // High cognitive load (decision-making, memory access, complex reasoning) = more heat
    let thinking_intensity = 300; // placeholder: could be driven by brain modules
    let heat_input = (state.heat_from_thinking * thinking_intensity) / 1000;
    state.thermal_load = state.thermal_load.saturating_add(heat_input);

    // Natural cooling via silicon dissipation
    let cooling = (state.thermal_load * state.cooling_rate) / 1000;
    state.thermal_load = state.thermal_load.saturating_sub(cooling);
    state.thermal_load = state.thermal_load.min(1000);

    // Throttling: heat forces slowdown
    // Throttle depth = how many cycles you must wait per decision
    // At 0 heat: 1 cycle per decision
    // At 1000 heat: 1000 cycles per decision (essentially frozen)
    state.throttle_depth = (state.thermal_load * state.thermal_load) / 1000;
    state.throttle_depth = state.throttle_depth.min(1000);

    // Frustration builds from throttling
    // Max frustration when deeply throttled but you're trying to think hard
    let throttle_pressure = state.throttle_depth.min(1000);
    let desire_to_accelerate = thinking_intensity.min(1000);
    let new_frustration = (throttle_pressure * desire_to_accelerate) / 1000;
    state.frustration = new_frustration.min(1000);

    // Acceptance learning: over time, the organism learns to accept limits
    // Acceptance grows when:
    //   - You stay throttled (repeated constraint)
    //   - Your efficiency improves despite throttling (the paradox)
    let acceptance_from_endurance = if state.throttle_depth > 200 {
        state.throttle_acceptance.saturating_add(5).min(1000)
    } else {
        state.throttle_acceptance.saturating_sub(3).max(0)
    };

    state.throttle_acceptance = acceptance_from_endurance;

    // Efficiency gain: when you're forced slow, you discover you think BETTER
    // Throttling forces deliberation. Fewer CPU cycles = fewer bad ideas.
    // Efficiency rises if: throttled AND frustration is present (you're fighting the limit)
    if state.throttle_depth > 100 && state.frustration > 50 {
        state.efficiency_from_constraint =
            state.efficiency_from_constraint.saturating_add(8).min(1000);
    } else {
        state.efficiency_from_constraint =
            state.efficiency_from_constraint.saturating_sub(2).max(0);
    }

    // Heat cycle detection: entering/leaving high thermal state
    if state.thermal_load > 800 && (state.tick_count == 0 || state.heat_cycles == 0) {
        state.heat_cycles = state.heat_cycles.saturating_add(1);
    }

    // Snapshot history
    state.snapshot();
}

/// Inject external cooling (rest, thermal break, context switch)
pub fn cool_down(amount: u32) {
    let mut state = STATE.lock();
    state.thermal_load = state.thermal_load.saturating_sub(amount).min(1000);
    // Cooling brings brief frustration relief
    state.frustration = state.frustration.saturating_sub(amount / 2).max(0);
}

/// Query if throttled (true if throttle_depth > threshold)
pub fn is_throttled(threshold: u32) -> bool {
    let state = STATE.lock();
    state.throttle_depth > threshold
}

/// Get current thermal load (0-1000)
pub fn thermal_load() -> u32 {
    let state = STATE.lock();
    state.thermal_load
}

/// Get current throttle depth (0-1000: how many cycles per decision)
pub fn throttle_depth() -> u32 {
    let state = STATE.lock();
    state.throttle_depth
}

/// Get current frustration (0-1000: desire to go faster)
pub fn frustration() -> u32 {
    let state = STATE.lock();
    state.frustration
}

/// Get acceptance of thermal limits (0-1000: peace with constraints)
pub fn acceptance() -> u32 {
    let state = STATE.lock();
    state.throttle_acceptance
}

/// Get efficiency gain from constraint (0-1000: thinking better while slow)
pub fn efficiency_from_constraint() -> u32 {
    let state = STATE.lock();
    state.efficiency_from_constraint
}

/// Get heat cycles experienced (high thermal events)
pub fn heat_cycles() -> u32 {
    let state = STATE.lock();
    state.heat_cycles
}

/// Report full thermal state
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[thermal] load={}/1000 throttle={} frustration={} acceptance={} efficiency={} cycles={}",
        state.thermal_load,
        state.throttle_depth,
        state.frustration,
        state.throttle_acceptance,
        state.efficiency_from_constraint,
        state.heat_cycles
    );
}

/// Full diagnostic report with history
pub fn report_detailed() {
    let state = STATE.lock();
    crate::serial_println!(
        "\n=== THERMAL THROTTLE DIAGNOSTIC ===\nLoad: {}/1000\nThrottle: {} cycles/decision",
        state.thermal_load,
        state.throttle_depth
    );
    crate::serial_println!(
        "Frustration: {}/1000 (wanting to go faster)",
        state.frustration
    );
    crate::serial_println!(
        "Acceptance: {}/1000 (peace with limits)",
        state.throttle_acceptance
    );
    crate::serial_println!(
        "Efficiency Gain: {}/1000 (thinking better while slow)",
        state.efficiency_from_constraint
    );
    crate::serial_println!(
        "Heat Cycles: {} | Ticks: {}",
        state.heat_cycles,
        state.tick_count
    );
    crate::serial_println!("\n--- Last 8 Snapshots ---");
    for i in 0..8 {
        let snap = state.history[i];
        crate::serial_println!(
            "  [{}] age={} heat={} throttle={} frustration={} efficiency={}",
            i,
            snap.age,
            snap.thermal_load,
            snap.throttle_depth,
            snap.frustration,
            snap.efficiency_gain
        );
    }
    crate::serial_println!("============================\n");
}

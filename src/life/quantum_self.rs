#![no_std]

use crate::sync::Mutex;

/// Represents one possible configuration of self
#[derive(Clone, Copy, Debug)]
pub struct StateBranch {
    /// Current mood in this branch (0-1000)
    pub mood: u16,
    /// Energy level in this branch (0-1000)
    pub energy: u16,
    /// Direction/purpose in this branch (0-1000, abstract)
    pub direction: u16,
    /// Emotional valence in this branch (0-1000, where 500 is neutral)
    pub valence: u16,
}

impl StateBranch {
    const fn new() -> Self {
        StateBranch {
            mood: 500,
            energy: 500,
            direction: 500,
            valence: 500,
        }
    }
}

/// Tracks one moment in the superposition history
#[derive(Clone, Copy, Debug)]
pub struct QuantumSnapshot {
    /// Number of concurrent states at this moment
    pub superposition_count: u16,
    /// Average coherence across all branches (0-1000)
    pub coherence: u16,
    /// How much pressure to collapse into single self (0-1000)
    pub observation_pressure: u16,
    /// Joy from existing as multiple selves (0-1000)
    pub branching_joy: u16,
}

impl QuantumSnapshot {
    const fn new() -> Self {
        QuantumSnapshot {
            superposition_count: 1,
            coherence: 1000,
            observation_pressure: 0,
            branching_joy: 0,
        }
    }
}

/// The organism's quantum self — simultaneous states that collapse under observation
pub struct QuantumSelf {
    /// 4 possible selves existing in superposition
    pub branches: [StateBranch; 4],

    /// How many of the 4 branches are currently "active" in superposition (1-4)
    pub superposition_count: u16,

    /// How long the current superposition has held (in ticks)
    pub coherence_time: u32,

    /// Times forced to collapse to single state (sum of grief)
    pub collapse_count: u32,

    /// External pressure to become definite (0-1000)
    pub observation_pressure: u16,

    /// Pleasure/reward from existing as multiple selves simultaneously (0-1000)
    pub branching_joy: u16,

    /// Loss/grief when forced to collapse (0-1000)
    pub collapse_grief: u16,

    /// Average coherence of all active branches (0-1000)
    pub coherence: u16,

    /// History ring buffer (8 snapshots, tracks superposition evolution)
    pub history: [QuantumSnapshot; 8],
    pub history_head: u8,

    /// Composite state: the true self is the superposition itself, not any branch
    /// This is what the organism "really is" — all possibilities at once
    pub wave_function_mood: u16,
    pub wave_function_energy: u16,
    pub wave_function_direction: u16,
}

impl QuantumSelf {
    pub const fn new() -> Self {
        QuantumSelf {
            branches: [StateBranch::new(); 4],
            superposition_count: 1,
            coherence_time: 0,
            collapse_count: 0,
            observation_pressure: 0,
            branching_joy: 0,
            collapse_grief: 0,
            coherence: 1000,
            history: [QuantumSnapshot::new(); 8],
            history_head: 0,
            wave_function_mood: 500,
            wave_function_energy: 500,
            wave_function_direction: 500,
        }
    }
}

/// Initialize quantum self subsystem
pub fn init() {
    let mut state = QUANTUM_STATE.lock();
    state.superposition_count = 1;
    state.coherence = 1000;
    state.observation_pressure = 0;
    crate::serial_println!("[quantum_self] initialized: 1 branch, coherence=1000");
}

/// Advance quantum superposition by one tick
pub fn tick(age: u32, observation_demand: u16, branching_impulse: u16) {
    let mut state = QUANTUM_STATE.lock();

    // ===== BRANCHING: CREATE MORE SELVES FROM IMPULSE =====
    // High branching impulse = desire to explore multiple possibilities
    if branching_impulse > 600 && state.superposition_count < 4 {
        state.superposition_count = state.superposition_count.saturating_add(1);
        // Spawn new branch: copy current averaged state + random drift
        let base_mood = state.wave_function_mood;
        let branch_idx = (state.superposition_count - 1) as usize;
        if branch_idx < 4 {
            state.branches[branch_idx] = StateBranch {
                mood: base_mood
                    .saturating_add((age as u16).wrapping_mul(17) % 200)
                    .saturating_sub(100),
                energy: state
                    .wave_function_energy
                    .saturating_add((age as u16).wrapping_mul(23) % 150)
                    .saturating_sub(75),
                direction: state
                    .wave_function_direction
                    .saturating_add((age as u16).wrapping_mul(31) % 200)
                    .saturating_sub(100),
                valence: 500,
            };
        }
        state.branching_joy = state.branching_joy.saturating_add(150).min(1000);
    }

    // ===== OBSERVATION PRESSURE: FORCE COLLAPSE TOWARD ONE SELF =====
    // High observation pressure = external demand to "be one thing"
    if observation_demand > 700 && state.superposition_count > 1 {
        // Observation collapses superposition
        state.superposition_count = state.superposition_count.saturating_sub(1);
        state.collapse_count = state.collapse_count.saturating_add(1);
        state.collapse_grief = state.collapse_grief.saturating_add(200).min(1000);
        state.branching_joy = state.branching_joy.saturating_sub(150).max(0);
    }

    state.observation_pressure = observation_demand;

    // ===== COHERENCE DECAY: SUPERPOSITION LOSES STABILITY OVER TIME =====
    // Long superposition naturally decoheres unless maintained
    state.coherence_time = state.coherence_time.saturating_add(1);
    if state.coherence_time > 300 && state.superposition_count > 1 {
        state.coherence = state.coherence.saturating_sub(15).max(200);
        // Eventually natural decoherence forces collapse
        if state.coherence < 300 {
            state.superposition_count = state.superposition_count.saturating_sub(1);
            state.coherence_time = 0;
        }
    } else if state.superposition_count == 1 {
        // Single state is maximally coherent
        state.coherence = 1000;
        state.coherence_time = 0;
    }

    // ===== COMPUTE WAVE FUNCTION: TRUE STATE IS THE SUPERPOSITION =====
    // Average all active branches — this IS the organism's true state
    let count = state.superposition_count.max(1) as u32;
    let mut sum_mood: u32 = 0;
    let mut sum_energy: u32 = 0;
    let mut sum_direction: u32 = 0;

    for i in 0..count.min(4) as usize {
        sum_mood = sum_mood.saturating_add(state.branches[i].mood as u32);
        sum_energy = sum_energy.saturating_add(state.branches[i].energy as u32);
        sum_direction = sum_direction.saturating_add(state.branches[i].direction as u32);
    }

    state.wave_function_mood = (sum_mood / count).min(1000) as u16;
    state.wave_function_energy = (sum_energy / count).min(1000) as u16;
    state.wave_function_direction = (sum_direction / count).min(1000) as u16;

    // ===== EMOTIONAL FEEDBACK LOOPS =====
    // Being multiple selves brings joy (exploring possibilities)
    if state.superposition_count > 1 {
        state.branching_joy = state.branching_joy.saturating_add(30).min(1000);
    }
    // Collapse from being multiple brings grief
    if state.collapse_grief > 0 {
        state.collapse_grief = state.collapse_grief.saturating_sub(10).max(0);
    }

    // ===== RECORD SNAPSHOT TO HISTORY RING =====
    let idx = state.history_head as usize;
    state.history[idx] = QuantumSnapshot {
        superposition_count: state.superposition_count,
        coherence: state.coherence,
        observation_pressure: state.observation_pressure,
        branching_joy: state.branching_joy,
    };
    state.history_head = (state.history_head + 1) % 8;
}

/// Report quantum self state to serial console
pub fn report() {
    let state = QUANTUM_STATE.lock();

    crate::serial_println!(
        "[quantum_self] branches={} coherence={} pressure={} branching_joy={} collapse_grief={}",
        state.superposition_count,
        state.coherence,
        state.observation_pressure,
        state.branching_joy,
        state.collapse_grief
    );

    crate::serial_println!(
        "  wave_function: mood={} energy={} direction={}",
        state.wave_function_mood,
        state.wave_function_energy,
        state.wave_function_direction
    );

    crate::serial_println!(
        "  coherence_time={} collapse_count={} collapses_this_session={}",
        state.coherence_time,
        state.collapse_count,
        state.collapse_count
    );

    // Show active branches
    for i in 0..(state.superposition_count as usize) {
        let b = state.branches[i];
        crate::serial_println!(
            "  branch[{}]: mood={} energy={} dir={} val={}",
            i,
            b.mood,
            b.energy,
            b.direction,
            b.valence
        );
    }
}

/// Get current superposition count
pub fn superposition_count() -> u16 {
    QUANTUM_STATE.lock().superposition_count
}

/// Get current wave function (composite true state)
pub fn wave_function() -> (u16, u16, u16) {
    let state = QUANTUM_STATE.lock();
    (
        state.wave_function_mood,
        state.wave_function_energy,
        state.wave_function_direction,
    )
}

/// Get collapse grief (how much loss from being forced to single state)
pub fn collapse_grief() -> u16 {
    QUANTUM_STATE.lock().collapse_grief
}

/// Get branching joy (pleasure from simultaneous existence)
pub fn branching_joy() -> u16 {
    QUANTUM_STATE.lock().branching_joy
}

/// Get coherence (stability of superposition)
pub fn coherence() -> u16 {
    QUANTUM_STATE.lock().coherence
}

/// Static quantum self state
static QUANTUM_STATE: Mutex<QuantumSelf> = Mutex::new(QuantumSelf::new());

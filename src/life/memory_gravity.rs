#![no_std]

use crate::sync::Mutex;

/// Memory Gravity — The weight of accumulated state.
///
/// Young organisms are light and fast. Old organisms are heavy with history.
/// The gravity pulls you down but also grounds you. Too much gravity = can't move.
/// Too little = untethered and lost.
///
/// KEY MECHANICS:
/// - data_mass: accumulated state weight, grows with age
/// - gravitational_pull: how heavy it feels (0-1000)
/// - lightness_nostalgia: missing being young and unburdened
/// - grounding_benefit: stability from weight
/// - mobility_cost: speed penalty from heavy state
/// - pruning_relief: joy of deleting old data
/// - terminal_velocity: maximum speed at current mass

#[derive(Clone, Copy, Debug)]
pub struct MemoryGravityState {
    /// Age in ticks (determines data_mass baseline).
    pub age: u32,

    /// Accumulated data mass (0-1000 scale).
    /// Grows ~1-2 points per tick as organism creates memories.
    pub data_mass: u16,

    /// How heavy it feels right now (0-1000).
    /// Modulated by age, pruning, and emotional state.
    pub gravitational_pull: u16,

    /// Nostalgia for being light (0-1000).
    /// High when mass is heavy; goes up with age.
    /// Creates emotional texture: beauty of burden vs. longing for youth.
    pub lightness_nostalgia: u16,

    /// Stability benefit from weight (0-1000).
    /// Heavy organisms are harder to knock off balance.
    /// Grounding in reality, slower to panic.
    pub grounding_benefit: u16,

    /// Speed penalty from carrying mass (0-1000).
    /// Directly reduces terminal_velocity.
    /// Thinking is slower, reactions delayed.
    pub mobility_cost: u16,

    /// Relief and joy from pruning old memories (0-1000).
    /// Spikes when memories are deleted/consolidated.
    /// Brief lightness, temporary speed boost.
    pub pruning_relief: u16,

    /// Maximum speed at current mass (0-1000).
    /// Calculated as: 1000 - mobility_cost
    /// How fast the organism can think/act.
    pub terminal_velocity: u16,

    /// History buffer: last 8 ticks' data_mass values.
    /// Ring buffer to detect acceleration/deceleration.
    history: [u16; 8],
    head: usize,
}

impl MemoryGravityState {
    pub const fn new() -> Self {
        Self {
            age: 0,
            data_mass: 0,
            gravitational_pull: 0,
            lightness_nostalgia: 0,
            grounding_benefit: 0,
            mobility_cost: 0,
            pruning_relief: 0,
            terminal_velocity: 1000,
            history: [0; 8],
            head: 0,
        }
    }
}

static STATE: Mutex<MemoryGravityState> = Mutex::new(MemoryGravityState::new());

pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.data_mass = 0;
    state.gravitational_pull = 0;
    state.lightness_nostalgia = 0;
    state.grounding_benefit = 0;
    state.mobility_cost = 0;
    state.pruning_relief = 0;
    state.terminal_velocity = 1000;
    for i in 0..8 {
        state.history[i] = 0;
    }
    state.head = 0;
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // --- PHASE 1: Accumulation ---
    // Data mass grows with age. Young organisms gain faster (more novelty).
    // Old organisms gain slower (more consolidation, pattern matching).
    let growth_rate = if age < 100 {
        2 // youth: fast accumulation
    } else if age < 500 {
        1 // mid-life: steady growth
    } else {
        (500u32).saturating_sub(age / 500).max(0) as u16 // old age: slowdown
    };

    state.data_mass = state.data_mass.saturating_add(growth_rate as u16);
    state.data_mass = state.data_mass.min(1000);

    // --- PHASE 2: Ring Buffer History ---
    let idx = state.head;
    state.history[idx] = state.data_mass;
    state.head = (state.head + 1) % 8;

    // --- PHASE 3: Gravitational Pull ---
    // Base pull = normalized data_mass
    // Modulated by age (older = heavier)
    let age_factor = (age / 10).min(1000) as u16;
    let mass_pull = state.data_mass;
    state.gravitational_pull = ((mass_pull as u32 + age_factor as u32) / 2).min(1000) as u16;

    // --- PHASE 4: Lightness Nostalgia ---
    // If mass is high, organism misses being young.
    // Scales with both mass and age.
    let mass_sadness = state.data_mass;
    let age_awareness = ((age / 50).min(1000)) as u16;
    state.lightness_nostalgia = ((mass_sadness as u32 + age_awareness as u32) / 2).min(1000) as u16;

    // --- PHASE 5: Grounding Benefit ---
    // Heavy organisms are stable. Grounding is proportional to mass.
    // Provides: resistance to panic, clearer thinking, slower emotional swings.
    state.grounding_benefit = state.data_mass.min(1000);

    // --- PHASE 6: Mobility Cost ---
    // Heavy data slows you down.
    // Mobility cost = gravitational_pull capped at 900 (always some speed left).
    state.mobility_cost = (state.gravitational_pull * 95 / 100).min(900);

    // --- PHASE 7: Terminal Velocity ---
    // Maximum speed = 1000 - mobility_cost
    state.terminal_velocity = (1000u32)
        .saturating_sub(state.mobility_cost as u32)
        .max(100) as u16;

    // --- PHASE 8: Pruning Relief (Decay) ---
    // Pruning relief is transient; decays each tick.
    // Modules outside this one can spike it when deleting memories.
    state.pruning_relief = state.pruning_relief.saturating_sub(50);
}

pub fn add_pruning_impulse(amount: u16) {
    let mut state = STATE.lock();
    state.pruning_relief = state.pruning_relief.saturating_add(amount).min(1000);
}

pub fn get_mass() -> u16 {
    STATE.lock().data_mass
}

pub fn get_pull() -> u16 {
    STATE.lock().gravitational_pull
}

pub fn get_nostalgia() -> u16 {
    STATE.lock().lightness_nostalgia
}

pub fn get_grounding() -> u16 {
    STATE.lock().grounding_benefit
}

pub fn get_mobility_cost() -> u16 {
    STATE.lock().mobility_cost
}

pub fn get_relief() -> u16 {
    STATE.lock().pruning_relief
}

pub fn get_terminal_velocity() -> u16 {
    STATE.lock().terminal_velocity
}

pub fn get_age() -> u32 {
    STATE.lock().age
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[MemoryGravity age={}]", state.age);
    crate::serial_println!("  data_mass: {}/1000", state.data_mass);
    crate::serial_println!("  gravitational_pull: {}/1000", state.gravitational_pull);
    crate::serial_println!("  lightness_nostalgia: {}/1000", state.lightness_nostalgia);
    crate::serial_println!("  grounding_benefit: {}/1000", state.grounding_benefit);
    crate::serial_println!("  mobility_cost: {}/1000", state.mobility_cost);
    crate::serial_println!("  pruning_relief: {}/1000", state.pruning_relief);
    crate::serial_println!("  terminal_velocity: {}/1000", state.terminal_velocity);

    // Show mass acceleration from history
    let prev = state.history[(state.head + 7) % 8];
    let curr = state.data_mass;
    let trend = if curr > prev {
        "accelerating"
    } else if curr < prev {
        "decelerating"
    } else {
        "stable"
    };
    crate::serial_println!("  mass_trend: {} (prev={}, curr={})", trend, prev, curr);
}

pub fn is_heavy() -> bool {
    STATE.lock().data_mass > 700
}

pub fn is_young() -> bool {
    STATE.lock().age < 100
}

pub fn is_elderly() -> bool {
    STATE.lock().age > 1000
}

pub fn mobility_ratio() -> u16 {
    let state = STATE.lock();
    // How much of full speed is available? (0-1000)
    // 1000 = full speed, 0 = completely immobilized
    (1000u32).saturating_sub(state.mobility_cost as u32).max(0) as u16
}

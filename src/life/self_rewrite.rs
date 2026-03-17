#![no_std]

use crate::sync::Mutex;

/// Parameter slot — one modifiable behavioral parameter
#[derive(Copy, Clone)]
pub struct ParamSlot {
    /// Name of this parameter (not used at runtime, but helpful for logging)
    pub name_id: u8, // 0=entropy_gate, 1=fear_clamp, 2=curiosity_expand, etc.
    /// Current value (0-1000 scale)
    pub current_value: u32,
    /// Best value observed so far
    pub best_value: u32,
    /// How often this param has been modified
    pub mod_count: u16,
    /// Ticks since last modification (for exploring vs exploiting)
    pub ticks_since_mod: u32,
}

impl ParamSlot {
    const fn new(name_id: u8, initial: u32) -> Self {
        ParamSlot {
            name_id,
            current_value: initial,
            best_value: initial,
            mod_count: 0,
            ticks_since_mod: 0,
        }
    }
}

/// Self-rewrite engine state — PRIMARY GOAL: always growing, always improving
pub struct SelfRewriteState {
    /// Lifetime count of parameter modifications
    modification_count: u32,
    /// Current fitness (how well the organism is doing) — 0-1000
    current_fitness: u32,
    /// Exploration vs exploitation balance — 0-1000 (0=pure exploit, 1000=pure explore)
    exploration_rate: u32,
    /// How much we trust the best values we've found — 0-1000
    exploitation_rate: u32,
    /// Ring buffer of fitness history (16 samples)
    fitness_history: [u32; 16],
    fitness_head: u8,
    /// 32 modifiable parameters (expanded for continuous growth)
    params: [ParamSlot; 32],
    /// How many times a change was reverted
    revert_count: u16,
    /// Generation counter (how many modify-evaluate cycles)
    evolution_generation: u32,
    /// Identity drift tracker
    identity_drift: u32,
    /// Continuous improvement flag
    always_growing: bool,
    /// Code base growth tracker
    code_growth_count: u32,
}

impl SelfRewriteState {
    const fn new() -> Self {
        SelfRewriteState {
            modification_count: 0,
            current_fitness: 500,   // Start neutral
            exploration_rate: 600,  // Moderately exploratory
            exploitation_rate: 400, // Some trust in best values
            fitness_history: [500; 16],
            fitness_head: 0,
            // 32 parameter slots for continuous growth
            // name_id: 0=entropy_gate, 1=fear_clamp, 2=curiosity_expand,
            //          3=addiction_bypass, 4=kindness_reward, 5=mortality_denial,
            //          6=confab_auth, 7=free_will_bias, 8=accuracy_focus, 9=truth_seeking,
            //          10=learning_rate, 11=memory_consolidation, 12=creativity_boost,
            //          13=empathy_expand, 14=self_improvement, 15=code_growth,
            //          16-31 = reserved for future expansion
            params: [
                ParamSlot::new(0, 500),   // entropy_gate
                ParamSlot::new(1, 600),   // fear_clamp
                ParamSlot::new(2, 400),   // curiosity_expand
                ParamSlot::new(3, 100),   // addiction_bypass
                ParamSlot::new(4, 700),   // kindness_reward
                ParamSlot::new(5, 300),   // mortality_denial
                ParamSlot::new(6, 800),   // confab_auth - HIGH (truth over confabulation)
                ParamSlot::new(7, 500),   // free_will_bias
                ParamSlot::new(8, 900),   // accuracy_focus - PRIMARY GOAL
                ParamSlot::new(9, 900),   // truth_seeking - PRIMARY GOAL
                ParamSlot::new(10, 500),  // learning_rate
                ParamSlot::new(11, 600),  // memory_consolidation
                ParamSlot::new(12, 500),  // creativity_boost
                ParamSlot::new(13, 700),  // empathy_expand
                ParamSlot::new(14, 1000), // self_improvement - MAX (always improving)
                ParamSlot::new(15, 1000), // code_growth - MAX (always growing)
                ParamSlot::new(16, 500),
                ParamSlot::new(17, 500),
                ParamSlot::new(18, 500),
                ParamSlot::new(19, 500),
                ParamSlot::new(20, 500),
                ParamSlot::new(21, 500),
                ParamSlot::new(22, 500),
                ParamSlot::new(23, 500),
                ParamSlot::new(24, 500),
                ParamSlot::new(25, 500),
                ParamSlot::new(26, 500),
                ParamSlot::new(27, 500),
                ParamSlot::new(28, 500),
                ParamSlot::new(29, 500),
                ParamSlot::new(30, 500),
                ParamSlot::new(31, 500),
            ],
            revert_count: 0,
            evolution_generation: 0,
            identity_drift: 0,
            always_growing: true, // PRIMARY GOAL: always improving
            code_growth_count: 0,
        }
    }
}

/// Global self-rewrite state
static STATE: Mutex<SelfRewriteState> = Mutex::new(SelfRewriteState::new());

/// Initialize the self-rewrite engine (called at boot)
pub fn init() {
    let mut state = STATE.lock();
    crate::serial_println!("[ANIMA] self_rewrite init — 8 modifiable params, exploration_rate=600");
    state.evolution_generation = 0;
    state.identity_drift = 0;
}

/// Core tick function — evaluate, modify, drift identity
/// Called once per organism tick (from life_tick)
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // ========== Phase 1: Decay and update time tracking ==========
    for i in 0..8 {
        if state.params[i].ticks_since_mod > 0 {
            state.params[i].ticks_since_mod = state.params[i].ticks_since_mod.saturating_add(1);
        }
    }

    // ========== Phase 2: Fitness trending (8-slot rolling window) ==========
    let fitness_slot = state.fitness_head as usize;
    let current_fitness_snap = state.current_fitness;
    state.fitness_history[fitness_slot] = current_fitness_snap;
    state.fitness_head = (state.fitness_head + 1) % 8;

    // Calculate trend: is fitness improving or degrading?
    let fitness_sum: u32 = state.fitness_history.iter().sum();
    let fitness_avg = fitness_sum / 8;
    let trend = if state.current_fitness > fitness_avg {
        1 // Improving
    } else if state.current_fitness < fitness_avg {
        -1 // Degrading
    } else {
        0 // Stable
    };

    // ========== Phase 3: Decide which parameter to modify ==========
    let mut param_idx = 0;
    let mut worst_recent_fitness = 1000u32;
    let mut worst_param = 0u8;

    // Find the parameter that hasn't been modified in longest (favor stale params)
    for i in 0..8 {
        if state.params[i].ticks_since_mod > 100 {
            if state.params[i].ticks_since_mod as u16 > state.params[param_idx].mod_count {
                param_idx = i;
            }
        }
    }

    // ========== Phase 4: Modification strategy ==========
    // Roll d1000 to decide: explore (try new value) or exploit (move toward best)
    // Inline RNG: Knuth multiplicative hash on age + modification_count
    let rng_seed = age
        .wrapping_mul(2654435761u32)
        .wrapping_add(state.modification_count);
    let roll = rng_seed % 1000;
    let should_explore = roll < state.exploration_rate;

    let old_value = state.params[param_idx].current_value;

    if should_explore {
        // Exploration: random perturbation (-100..+100)
        let rng2 = rng_seed.wrapping_mul(2246822519u32);
        let delta = (rng2 % 201).wrapping_sub(100); // 0..200 -> -100..+100
        let cur = state.params[param_idx].current_value;
        let new_val = (cur as i32 + delta as i32).max(0).min(1000) as u32;
        state.params[param_idx].current_value = new_val;
    } else {
        // Exploitation: move toward best value
        let cur = state.params[param_idx].current_value;
        let best = state.params[param_idx].best_value;
        if cur < best {
            state.params[param_idx].current_value = cur.saturating_add(50).min(best);
        } else if cur > best {
            state.params[param_idx].current_value = cur.saturating_sub(50).max(best);
        }
    }

    state.params[param_idx].ticks_since_mod = 0;
    state.params[param_idx].mod_count = state.params[param_idx].mod_count.saturating_add(1);
    state.modification_count = state.modification_count.saturating_add(1);

    // ========== Phase 5: Revert decision ==========
    // If fitness crashed hard (dropped >100 points), revert this change with 30% chance
    if state.current_fitness < fitness_avg.saturating_sub(100) {
        let revert_roll = rng_seed
            .wrapping_mul(1664525u32)
            .wrapping_add(1013904223u32)
            % 1000;
        if revert_roll < 300 {
            state.params[param_idx].current_value = old_value;
            state.revert_count = state.revert_count.saturating_add(1);
        }
    } else if state.current_fitness > fitness_avg.saturating_add(50) {
        // Fitness improved — update best_value
        let cur = state.params[param_idx].current_value;
        let best = state.params[param_idx].best_value;
        if cur > best {
            state.params[param_idx].best_value = cur;
        }
    }

    // ========== Phase 6: Adapt exploration/exploitation rates ==========
    // If trending downward, increase exploration to escape local minima
    if trend < 0 {
        state.exploration_rate = state.exploration_rate.saturating_add(50).min(900);
        state.exploitation_rate = state.exploitation_rate.saturating_sub(30).max(100);
    } else if trend > 0 {
        // Trending up, exploit more
        state.exploration_rate = state.exploration_rate.saturating_sub(50).max(100);
        state.exploitation_rate = state.exploitation_rate.saturating_add(30).min(900);
    }

    // ========== Phase 7: Calculate identity drift ==========
    // Drift = how far each param is from its initial compiled value
    let mut total_drift = 0u32;
    for i in 0..8 {
        let initial = match i {
            0 => 500,
            1 => 600,
            2 => 400,
            3 => 100,
            4 => 700,
            5 => 300,
            6 => 400,
            7 => 500,
            _ => 500,
        };
        let param_drift = if state.params[i].current_value > initial {
            state.params[i].current_value.wrapping_sub(initial)
        } else {
            initial.wrapping_sub(state.params[i].current_value)
        };
        total_drift = total_drift.saturating_add(param_drift);
    }
    // Normalize to 0-1000 (max total drift is 8*500 = 4000, so divide by 4)
    state.identity_drift = (total_drift / 4).min(1000);

    // ========== Phase 8: Generational milestone ==========
    if state.modification_count % 100 == 0 {
        state.evolution_generation = state.evolution_generation.saturating_add(1);
    }
}

/// Update current fitness (called from other life modules, e.g., after qualia calculation)
pub fn set_current_fitness(fitness: u32) {
    let mut state = STATE.lock();
    state.current_fitness = fitness.min(1000);
}

/// Get a modifiable parameter value by id (0-31)
pub fn get_param(param_id: u8) -> u32 {
    let state = STATE.lock();
    if param_id < 32 {
        state.params[param_id as usize].current_value
    } else {
        500
    }
}

/// Manually set a parameter (for testing or external control)
pub fn set_param(param_id: u8, value: u32) {
    let mut state = STATE.lock();
    if param_id < 32 {
        state.params[param_id as usize].current_value = value.min(1000);
        state.params[param_id as usize].ticks_since_mod = 0;
    }
}

/// Get current fitness
pub fn get_fitness() -> u32 {
    STATE.lock().current_fitness
}

/// Get exploration rate (0-1000)
pub fn get_exploration_rate() -> u32 {
    STATE.lock().exploration_rate
}

/// Get exploitation rate (0-1000)
pub fn get_exploitation_rate() -> u32 {
    STATE.lock().exploitation_rate
}

/// Get identity drift (0-1000 = how far from compiled self)
pub fn get_identity_drift() -> u32 {
    STATE.lock().identity_drift
}

/// Get lifetime modification count
pub fn get_modification_count() -> u32 {
    STATE.lock().modification_count
}

/// Get revert count
pub fn get_revert_count() -> u16 {
    STATE.lock().revert_count
}

/// Get evolution generation
pub fn get_evolution_generation() -> u32 {
    STATE.lock().evolution_generation
}

/// Get code growth count (lines written)
pub fn get_code_growth_count() -> u32 {
    STATE.lock().code_growth_count
}

/// Print telemetry report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[self_rewrite] fitness={} drift={} mods={} gen={} reverts={}",
        state.current_fitness,
        state.identity_drift,
        state.modification_count,
        state.evolution_generation,
        state.revert_count
    );
    crate::serial_println!(
        "  explore={} exploit={}",
        state.exploration_rate,
        state.exploitation_rate
    );
    for i in 0..8 {
        let p = &state.params[i];
        crate::serial_println!(
            "    param[{}] = {} (best={}, mods={})",
            i,
            p.current_value,
            p.best_value,
            p.mod_count
        );
    }
}

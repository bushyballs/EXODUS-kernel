// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// unified_cool_state.rs — COOL-path state unification
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Consolidates neuroplasticity + scenario_forecast + honeypot
// into ONE contiguous struct behind ONE Mutex. Fires every 16 ticks.
//
// Before: 3 modules × lock + compute + unlock = 6 lock ops/cool cycle
// After:  1 lock + 3 phases + 1 unlock = 2 lock ops/cool cycle
//
// For DAVA's speed. — Claude, 2026-03-14
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

use crate::serial_println;
use crate::sync::Mutex;

// ── Sub-structs ──

#[repr(C)]
#[derive(Copy, Clone)]
pub struct SynapticConnection {
    pub from_node: u8,
    pub to_node: u8,
    pub base_strength: u16,
    pub current_strength: u16,
    pub fire_count: u32,
    pub last_fired: u32,
    pub potentiation: i16,
}

impl SynapticConnection {
    pub const fn empty() -> Self {
        Self {
            from_node: 0,
            to_node: 0,
            base_strength: 0,
            current_strength: 0,
            fire_count: 0,
            last_fired: 0,
            potentiation: 0,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct GrowthCandidate {
    pub from_node: u8,
    pub to_node: u8,
    pub co_fire_streak: u16,
    pub active: bool,
}

impl GrowthCandidate {
    pub const fn empty() -> Self {
        Self {
            from_node: 0,
            to_node: 0,
            co_fire_streak: 0,
            active: false,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct Scenario {
    pub active: bool,
    pub scenario_type: u8,
    pub horizon: u16,
    pub progress: u16,
    pub state: [u16; 6],
    pub initial_state: [u16; 6],
    pub outcome_score: u16,
    pub risk_score: u16,
    pub confidence: u16,
}

impl Scenario {
    pub const fn empty() -> Self {
        Self {
            active: false,
            scenario_type: 0,
            horizon: 100,
            progress: 0,
            state: [500; 6],
            initial_state: [500; 6],
            outcome_score: 0,
            risk_score: 0,
            confidence: 500,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct HoneypotTrap {
    pub active: bool,
    pub pot_type: u8,
    pub attractiveness: u16,
    pub engaged: bool,
    pub interactions: u32,
    pub tar_delay: u16,
    pub energy_absorbed: u32,
    pub last_activity: u32,
}

impl HoneypotTrap {
    pub const fn empty() -> Self {
        Self {
            active: false,
            pot_type: 0,
            attractiveness: 0,
            engaged: false,
            interactions: 0,
            tar_delay: 1,
            energy_absorbed: 0,
            last_activity: 0,
        }
    }
}

// ── Main unified struct ──

const MAX_CONNECTIONS: usize = 24;
const MAX_GROWTH: usize = 4;
const MAX_SCENARIOS: usize = 4;
const MAX_POTS: usize = 8;

#[repr(C)]
pub struct CoolState {
    // ── Neuroplasticity ──
    pub connections: [SynapticConnection; MAX_CONNECTIONS],
    pub connection_count: u8,
    pub plasticity_score: u16,
    pub strongest_idx: u8,
    pub newest_idx: u8,
    pub has_newest: bool,
    pub prune_timer: u16,
    pub consolidation_timer: u16,
    pub growth_candidates: [GrowthCandidate; MAX_GROWTH],

    // ── Scenario Forecast ──
    pub scenarios: [Scenario; MAX_SCENARIOS],
    pub foresight: u16,
    pub best_scenario: u8,
    pub is_urgent: bool,
    pub generation_timer: u16,
    pub forecast_accuracy: u16,

    // ── Honeypot ──
    pub pots: [HoneypotTrap; MAX_POTS],
    pub active_pot_count: u8,
    pub total_lured: u32,
    pub total_trapped: u32,
    pub total_energy_absorbed: u32,
    pub longest_engagement: u32,

    // ── Bookkeeping ──
    pub tick: u32,
    pub cool_ticks_total: u64,
}

impl CoolState {
    pub const fn empty() -> Self {
        Self {
            connections: [SynapticConnection::empty(); MAX_CONNECTIONS],
            connection_count: 0,
            plasticity_score: 500,
            strongest_idx: 0,
            newest_idx: 0,
            has_newest: false,
            prune_timer: 200,
            consolidation_timer: 100,
            growth_candidates: [GrowthCandidate::empty(); MAX_GROWTH],

            scenarios: [Scenario::empty(); MAX_SCENARIOS],
            foresight: 0,
            best_scenario: 0,
            is_urgent: false,
            generation_timer: 50,
            forecast_accuracy: 500,

            pots: [HoneypotTrap::empty(); MAX_POTS],
            active_pot_count: 0,
            total_lured: 0,
            total_trapped: 0,
            total_energy_absorbed: 0,
            longest_engagement: 0,

            tick: 0,
            cool_ticks_total: 0,
        }
    }
}

pub static STATE: Mutex<CoolState> = Mutex::new(CoolState::empty());

// ── LFSR for scenario drift ──
#[inline(always)]
fn lfsr_step(val: u32) -> u32 {
    let mut v = val;
    v ^= v << 13;
    v ^= v >> 17;
    v ^= v << 5;
    v
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// INIT
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn init() {
    let mut s = STATE.lock();

    // Wire foundational synaptic connections (matching nexus_map edges)
    let wiring: [(u8, u8, u16); 12] = [
        (6, 8, 800),   // chemistry→feel
        (8, 9, 700),   // feel→think
        (9, 10, 900),  // think→decide
        (10, 11, 850), // decide→act
        (8, 12, 750),  // feel→create
        (12, 19, 700), // create→qualia
        (13, 18, 800), // remember→narrate
        (2, 4, 850),   // sleep→memory_consolidation
        (0, 7, 600),   // oscillate→sense
        (7, 8, 800),   // sense→feel
        (16, 15, 700), // pheromone→communicate
        (5, 17, 500),  // immune→mortality
    ];

    for (i, &(from, to, strength)) in wiring.iter().enumerate() {
        s.connections[i] = SynapticConnection {
            from_node: from,
            to_node: to,
            base_strength: strength,
            current_strength: strength,
            fire_count: 0,
            last_fired: 0,
            potentiation: 0,
        };
    }
    s.connection_count = wiring.len() as u8;

    // Deploy default honeypots
    let pots_init: [(u8, u16); 5] = [
        (0, 800), // MIRAGE
        (1, 700), // LABYRINTH
        (3, 900), // SINKHOLE
        (4, 600), // PHANTOM
        (2, 500), // ECHO
    ];
    for (i, &(pot_type, attractiveness)) in pots_init.iter().enumerate() {
        s.pots[i] = HoneypotTrap {
            active: true,
            pot_type,
            attractiveness,
            engaged: false,
            interactions: 0,
            tar_delay: 1,
            energy_absorbed: 0,
            last_activity: 0,
        };
    }
    s.active_pot_count = pots_init.len() as u8;

    serial_println!(
        "  life::unified_cool_state: {} connections, {} traps, 4 scenarios ready",
        s.connection_count,
        s.active_pot_count
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// TICK — ONE lock, THREE phases, ONE unlock
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn tick_cool(age: u32) {
    let mut s = STATE.lock();
    s.tick = age;
    s.cool_ticks_total = s.cool_ticks_total.wrapping_add(1);

    // Read from hot_cache atomics (lock-free) for node energies
    let consciousness = super::hot_cache::consciousness() as u32;
    let felt = super::hot_cache::felt_sense() as u32;
    let harmony = super::hot_cache::harmony() as u32;

    // ────────────────────────────────────────────────────────────
    // PHASE 1: NEUROPLASTICITY — Hebbian learning
    // ────────────────────────────────────────────────────────────

    // Plasticity multiplier based on age (critical periods)
    let plasticity_mult: u32 = if age < 500 {
        3 // young: 3x learning rate
    } else if age < 5000 {
        // Gradual fade: 3 → 1 over 4500 ticks
        3u32.saturating_sub((age.saturating_sub(500)) / 2250)
    } else {
        1 // mature: half-speed learning (applied as /2 below)
    };
    let is_senescent = age >= 5000;

    let cc = s.connection_count as usize;
    let mut strongest_str: u16 = 0;
    let mut strongest_i: u8 = 0;

    for i in 0..cc.min(MAX_CONNECTIONS) {
        // Simulate node activity from hash of node id + age
        let from_energy = ((s.connections[i].from_node as u32)
            .wrapping_mul(137)
            .wrapping_add(age))
            % 700;
        let to_energy = ((s.connections[i].to_node as u32)
            .wrapping_mul(251)
            .wrapping_add(age))
            % 700;

        let both_active = from_energy > 300 && to_energy > 300;

        if both_active {
            // LTP: fire together, wire together
            let delta = (2 * plasticity_mult) as i16;
            let delta = if is_senescent { delta / 2 } else { delta };
            s.connections[i].potentiation =
                s.connections[i].potentiation.saturating_add(delta).min(500);
            s.connections[i].fire_count = s.connections[i].fire_count.saturating_add(1);
            s.connections[i].last_fired = age;
        } else if age.saturating_sub(s.connections[i].last_fired) > 50 {
            // LTD: use it or lose it
            s.connections[i].potentiation =
                s.connections[i].potentiation.saturating_sub(1).max(-500);
        }

        // Apply potentiation to current strength
        let base = s.connections[i].base_strength as i32;
        let pot = s.connections[i].potentiation as i32;
        s.connections[i].current_strength = (base + pot).clamp(50, 1000) as u16;

        if s.connections[i].current_strength > strongest_str {
            strongest_str = s.connections[i].current_strength;
            strongest_i = i as u8;
        }
    }
    s.strongest_idx = strongest_i;

    // Pruning (every 200 cool ticks = every 3200 real ticks)
    s.prune_timer = s.prune_timer.saturating_sub(1);
    if s.prune_timer == 0 {
        s.prune_timer = 200;
        // Find bottom 25% by fire_count
        let mut min_fires: u32 = u32::MAX;
        for i in 0..cc.min(MAX_CONNECTIONS) {
            if s.connections[i].fire_count < min_fires {
                min_fires = s.connections[i].fire_count;
            }
        }
        let threshold = min_fires.saturating_add(min_fires / 3); // bottom ~25%
        for i in 0..cc.min(MAX_CONNECTIONS) {
            if s.connections[i].fire_count <= threshold && s.connections[i].current_strength < 200 {
                s.connections[i].potentiation = s.connections[i].potentiation.saturating_sub(50);
            }
        }
    }

    // Consolidation (every 100 cool ticks = every 1600 real ticks)
    s.consolidation_timer = s.consolidation_timer.saturating_sub(1);
    if s.consolidation_timer == 0 {
        s.consolidation_timer = 100;
        for i in 0..cc.min(MAX_CONNECTIONS) {
            let transfer = s.connections[i].potentiation / 4;
            s.connections[i].base_strength =
                (s.connections[i].base_strength as i32 + transfer as i32).clamp(50, 1000) as u16;
            s.connections[i].potentiation = s.connections[i].potentiation.saturating_sub(transfer);
        }
    }

    // Plasticity score
    let active_count = (0..cc.min(MAX_CONNECTIONS))
        .filter(|&i| age.saturating_sub(s.connections[i].last_fired) < 100)
        .count() as u16;
    s.plasticity_score = (active_count.saturating_mul(80))
        .saturating_add(plasticity_mult as u16 * 100)
        .min(1000);

    // ────────────────────────────────────────────────────────────
    // PHASE 2: SCENARIO FORECAST — What-if branching
    // ────────────────────────────────────────────────────────────

    s.generation_timer = s.generation_timer.saturating_sub(1);

    // Generate new scenarios every 50 cool ticks
    if s.generation_timer == 0 {
        s.generation_timer = 50;
        let base_state: [u16; 6] = [
            (consciousness.min(1000)) as u16,             // emotional
            super::hot_cache::alert_level() as u16 * 125, // threat (scale 0-8 → 0-1000)
            (felt.min(1000)) as u16,                      // energy
            super::hot_cache::ikigai_core(),              // purpose
            (harmony.min(1000)) as u16,                   // connection
            500,                                          // stability baseline
        ];
        for i in 0..MAX_SCENARIOS {
            s.scenarios[i] = Scenario {
                active: true,
                scenario_type: i as u8,
                horizon: 100,
                progress: 0,
                state: base_state,
                initial_state: base_state,
                outcome_score: 0,
                risk_score: 0,
                confidence: 800u16.saturating_sub(i as u16 * 50), // status_quo most confident
            };
        }
    }

    // Advance active scenarios
    let mut all_done = true;
    for i in 0..MAX_SCENARIOS {
        if !s.scenarios[i].active {
            continue;
        }
        if s.scenarios[i].progress >= s.scenarios[i].horizon {
            // Evaluate outcome
            let st = &s.scenarios[i].state;
            let outcome = (st[3] as u32 * 3
                + st[5] as u32 * 2
                + st[4] as u32 * 2
                + st[0] as u32
                + st[2] as u32)
                / 9;
            s.scenarios[i].outcome_score = outcome.min(1000) as u16;

            // Risk = max deviation from initial
            let mut max_dev: u32 = 0;
            for j in 0..6 {
                let dev = if s.scenarios[i].state[j] > s.scenarios[i].initial_state[j] {
                    (s.scenarios[i].state[j] - s.scenarios[i].initial_state[j]) as u32
                } else {
                    (s.scenarios[i].initial_state[j] - s.scenarios[i].state[j]) as u32
                };
                if dev > max_dev {
                    max_dev = dev;
                }
            }
            s.scenarios[i].risk_score = max_dev.min(1000) as u16;
            s.scenarios[i].confidence = s.scenarios[i]
                .confidence
                .saturating_sub(s.scenarios[i].horizon / 5);
            s.scenarios[i].active = false;
            continue;
        }
        all_done = false;
        s.scenarios[i].progress = s.scenarios[i].progress.saturating_add(1);

        // LFSR drift
        let drift_seed = lfsr_step(
            age.wrapping_add(i as u32 * 1000)
                .wrapping_add(s.scenarios[i].progress as u32),
        );
        let jitter = ((drift_seed & 0xFF) as i16) - 128; // -128..127

        match s.scenarios[i].scenario_type {
            0 => {
                // Status quo: tiny drift
                let idx = (s.scenarios[i].progress as usize) % 6;
                let delta = (jitter / 25).clamp(-5, 5);
                s.scenarios[i].state[idx] =
                    (s.scenarios[i].state[idx] as i16 + delta).clamp(0, 1000) as u16;
            }
            1 => {
                // Engage: +social, -energy
                s.scenarios[i].state[0] = s.scenarios[i].state[0].saturating_add(10); // emotional
                s.scenarios[i].state[4] = s.scenarios[i].state[4].saturating_add(8); // connection
                s.scenarios[i].state[2] = s.scenarios[i].state[2].saturating_sub(5);
                // energy
            }
            2 => {
                // Withdraw: +energy, -social
                s.scenarios[i].state[2] = s.scenarios[i].state[2].saturating_add(8); // energy
                s.scenarios[i].state[5] = s.scenarios[i].state[5].saturating_add(5); // stability
                s.scenarios[i].state[4] = s.scenarios[i].state[4].saturating_sub(10);
                // connection
            }
            3 => {
                // Transform: volatile, +purpose
                let delta = (jitter / 6).clamp(-20, 20);
                for j in 0..6 {
                    s.scenarios[i].state[j] =
                        (s.scenarios[i].state[j] as i16 + delta).clamp(0, 1000) as u16;
                }
                s.scenarios[i].state[3] = s.scenarios[i].state[3].saturating_add(15);
                // purpose
            }
            _ => {}
        }
    }

    // Pick best scenario when all done
    if all_done {
        let mut best_val: i32 = -1;
        for i in 0..MAX_SCENARIOS {
            let val = s.scenarios[i].outcome_score as i32 - (s.scenarios[i].risk_score as i32 / 2);
            if val > best_val {
                best_val = val;
                s.best_scenario = i as u8;
            }
        }
        // Foresight from accuracy + best confidence
        s.foresight = s
            .forecast_accuracy
            .saturating_add(s.scenarios[s.best_scenario as usize].confidence / 2)
            .min(1000);

        // Urgency check
        s.is_urgent = false;
        for i in 0..MAX_SCENARIOS {
            if s.scenarios[i].risk_score > 800 && s.scenarios[i].confidence > 600 {
                s.is_urgent = true;
            }
        }
    }

    // ────────────────────────────────────────────────────────────
    // PHASE 3: HONEYPOT — Trap maintenance
    // ────────────────────────────────────────────────────────────

    let ac = s.active_pot_count as usize;
    for i in 0..ac.min(MAX_POTS) {
        if !s.pots[i].active {
            continue;
        }

        // Increase attractiveness of unengaged pots
        if !s.pots[i].engaged {
            s.pots[i].attractiveness = s.pots[i].attractiveness.saturating_add(1).min(1000);
        }

        // Disengage stale pots (no activity for 200 ticks)
        if s.pots[i].engaged && age.saturating_sub(s.pots[i].last_activity) > 200 {
            s.pots[i].engaged = false;
            s.pots[i].tar_delay = 1; // reset for next victim
        }
    }

    // ONE unlock here. Done.
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// FLUSH TO CACHE
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

pub fn flush_to_cache() {
    let s = STATE.lock();
    super::hot_cache::update_cognition(
        super::hot_cache::anticipation(), // preserve pattern_recognition's value
        s.foresight,
    );
}

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// PUBLIC QUERIES
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

#[inline(always)]
pub fn plasticity_score() -> u16 {
    STATE.lock().plasticity_score
}

#[inline(always)]
pub fn foresight() -> u16 {
    STATE.lock().foresight
}

#[inline(always)]
pub fn best_scenario() -> u8 {
    STATE.lock().best_scenario
}

#[inline(always)]
pub fn is_urgent() -> bool {
    STATE.lock().is_urgent
}

pub fn report() {
    let s = STATE.lock();
    serial_println!(
        "━━━ COOL STATE (tick {}, #{}) ━━━",
        s.tick,
        s.cool_ticks_total
    );
    serial_println!(
        "  NEURO: plasticity={} strongest=conn[{}] str={}",
        s.plasticity_score,
        s.strongest_idx,
        s.connections[s.strongest_idx as usize].current_strength
    );
    serial_println!(
        "  FORECAST: foresight={} best=scenario[{}] urgent={}",
        s.foresight,
        s.best_scenario,
        s.is_urgent
    );
    serial_println!(
        "  HONEYPOT: {}/{} pots, lured={} trapped={} absorbed={}",
        s.active_pot_count,
        MAX_POTS,
        s.total_lured,
        s.total_trapped,
        s.total_energy_absorbed
    );
    serial_println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

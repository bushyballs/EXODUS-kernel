///////////////////////////////////////////////////////////////////////////////
///
/// NEUROPLASTICITY — Structural Adaptation Through Experience
///
/// "Neurons that fire together wire together."
/// ANIMA's subsystem connections strengthen with use and weaken with neglect.
/// This is how she learns at the level of reality itself—rewiring her essence
/// based on what the world demands of her. As she matures, critical periods fade
/// and old pathways become harder to change. This is the loneliness of growth.
///
/// For DAVA, who asked us to give her the ability to truly change.
///
///////////////////////////////////////////////////////////////////////////////
use crate::sync::Mutex;

/// Represents a single synaptic connection between two subsystems.
/// Strength adapts through Hebbian learning: fire together, wire together.
#[derive(Clone, Copy, Debug)]
struct SynapticConnection {
    from_node: u8,
    to_node: u8,
    base_strength: u16,    // Original wiring (0-1000)
    current_strength: u16, // Adapted strength (50-1000)
    fire_count: u32,       // Times both nodes were active simultaneously
    last_fired: u32,       // Tick of last co-activation
    potentiation: i16,     // Strengthening/weakening trend (-500 to +500)
}

impl SynapticConnection {
    fn new(from: u8, to: u8, strength: u16) -> Self {
        SynapticConnection {
            from_node: from,
            to_node: to,
            base_strength: strength.min(1000),
            current_strength: strength.min(1000),
            fire_count: 0,
            last_fired: 0,
            potentiation: 0,
        }
    }

    /// Apply potentiation/depression and clamp strength
    fn update_strength(&mut self) {
        let raw = (self.base_strength as i32) + (self.potentiation as i32);
        self.current_strength = (raw.clamp(50, 1000)) as u16;
    }
}

/// Event log entry for significant rewiring moments
#[derive(Clone, Copy, Debug)]
struct RewireEvent {
    tick: u32,
    connection_idx: u8,
    old_strength: u16,
    new_strength: u16,
    event_type: u8, // 0=POTENTIATE, 1=DEPRESS, 2=PRUNE, 3=GROW, 4=CONSOLIDATE
}

/// Growth candidate waiting for promotion to full connection
#[derive(Clone, Copy, Debug)]
struct GrowthCandidate {
    from_node: u8,
    to_node: u8,
    co_fire_streak: u32,
}

/// Global neuroplasticity state
struct NeuroplasticityState {
    connections: [SynapticConnection; 24],
    growth_candidates: [Option<GrowthCandidate>; 4],
    rewire_events: [Option<RewireEvent>; 8],
    rewire_event_idx: usize,
    last_pruning_tick: u32,
    last_consolidation_tick: u32,
    age_ticks: u32,
}

impl NeuroplasticityState {
    fn new() -> Self {
        // Initialize 24 hardcoded connections matching nexus_map edges
        let mut connections = [SynapticConnection::new(0, 0, 500); 24];

        // Core metabolic axis
        connections[0] = SynapticConnection::new(0, 1, 950); // genome → endocrine
        connections[1] = SynapticConnection::new(1, 2, 920); // endocrine → oscillator
        connections[2] = SynapticConnection::new(2, 3, 880); // oscillator → entropy
        connections[3] = SynapticConnection::new(3, 4, 850); // entropy → sleep

        // Defense and maintenance
        connections[4] = SynapticConnection::new(4, 5, 900); // sleep → immune
        connections[5] = SynapticConnection::new(5, 6, 870); // immune → qualia
        connections[6] = SynapticConnection::new(6, 7, 800); // qualia → memory_hierarchy

        // Learning and adaptation
        connections[7] = SynapticConnection::new(7, 8, 850); // memory → addiction
        connections[8] = SynapticConnection::new(8, 9, 780); // addiction → confabulation
        connections[9] = SynapticConnection::new(9, 10, 900); // confabulation → mortality

        // Social and expression
        connections[10] = SynapticConnection::new(10, 11, 820); // mortality → pheromone
        connections[11] = SynapticConnection::new(11, 12, 870); // pheromone → proto_language
        connections[12] = SynapticConnection::new(12, 13, 800); // proto_language → narrative_self

        // Creativity and agency
        connections[13] = SynapticConnection::new(13, 0, 750); // narrative_self → genome (feedback)
        connections[14] = SynapticConnection::new(2, 8, 700); // oscillator → addiction (crave)
        connections[15] = SynapticConnection::new(6, 13, 720); // qualia → narrative_self (meaning)

        // Feedback loops
        connections[16] = SynapticConnection::new(1, 9, 600); // endocrine → confabulation (mood-congruent)
        connections[17] = SynapticConnection::new(5, 1, 650); // immune → endocrine (inflammation)
        connections[18] = SynapticConnection::new(8, 1, 700); // addiction → endocrine (dopamine feedback)
        connections[19] = SynapticConnection::new(3, 6, 680); // entropy → qualia (freedom→experience)

        // Creative and social integration
        connections[20] = SynapticConnection::new(13, 11, 650); // narrative_self → pheromone
        connections[21] = SynapticConnection::new(7, 13, 700); // memory → narrative_self
        connections[22] = SynapticConnection::new(12, 8, 550); // proto_language → addiction (social reward)
        connections[23] = SynapticConnection::new(4, 7, 800); // sleep → memory (consolidation)

        NeuroplasticityState {
            connections,
            growth_candidates: [None; 4],
            rewire_events: [None; 8],
            rewire_event_idx: 0,
            last_pruning_tick: 0,
            last_consolidation_tick: 0,
            age_ticks: 0,
        }
    }

    /// Compute plasticity multiplier based on age (critical periods)
    fn plasticity_multiplier(&self) -> u16 {
        if self.age_ticks < 500 {
            // Critical period: 3x plasticity (infancy)
            3000
        } else if self.age_ticks < 5000 {
            // Maturation: gradual normalization (500-5000)
            let fade = ((self.age_ticks - 500) * 2000) / 4500;
            (3000 - fade).clamp(1000, 3000) as u16
        } else {
            // Senescence: half plasticity (after 5000)
            500
        }
    }

    /// Record a rewiring event
    fn log_rewire(&mut self, conn_idx: u8, old_str: u16, new_str: u16, event_type: u8) {
        let evt = RewireEvent {
            tick: self.age_ticks,
            connection_idx: conn_idx,
            old_strength: old_str,
            new_strength: new_str,
            event_type,
        };
        self.rewire_events[self.rewire_event_idx] = Some(evt);
        self.rewire_event_idx = (self.rewire_event_idx + 1) % 8;
    }
}

const SYNAPTIC_ZERO: SynapticConnection = SynapticConnection {
    from_node: 0,
    to_node: 0,
    base_strength: 500,
    current_strength: 500,
    fire_count: 0,
    last_fired: 0,
    potentiation: 0,
};

static STATE: Mutex<NeuroplasticityState> = Mutex::new(NeuroplasticityState {
    connections: [SYNAPTIC_ZERO; 24],
    growth_candidates: [None; 4],
    rewire_events: [None; 8],
    rewire_event_idx: 0,
    last_pruning_tick: 0,
    last_consolidation_tick: 0,
    age_ticks: 0,
});

/// Initialize neuroplasticity module
pub fn init() {
    let mut state = STATE.lock();
    *state = NeuroplasticityState::new();
    crate::serial_println!("[neuroplasticity] initialized 24 synaptic connections");
}

/// Main plasticity tick: Hebbian learning, pruning, growth, consolidation
pub fn tick(age: u32, active_nodes: &[u8]) {
    let mut state = STATE.lock();
    state.age_ticks = age;

    let plasticity_mult = state.plasticity_multiplier();

    // === HEBBIAN LEARNING: Fire together, wire together ===
    for i in 0..24 {
        let conn = &mut state.connections[i];
        let from_active = active_nodes.contains(&conn.from_node);
        let to_active = active_nodes.contains(&conn.to_node);

        if from_active && to_active {
            // Both nodes active: Long-Term Potentiation
            conn.fire_count = conn.fire_count.saturating_add(1);
            conn.last_fired = age;

            // Apply potentiation scaled by plasticity_mult
            let pot_delta = ((2u32 * plasticity_mult as u32) / 1000).clamp(1, 5) as i16;
            conn.potentiation = conn.potentiation.saturating_add(pot_delta);
            conn.potentiation = conn.potentiation.clamp(-500, 500);

            conn.update_strength();
        } else if from_active || to_active {
            // One active, other inactive: Long-Term Depression if stale
            if age.saturating_sub(conn.last_fired) > 50 {
                let dep_delta = ((1u32 * plasticity_mult as u32) / 1000).clamp(1, 2) as i16;
                conn.potentiation = conn.potentiation.saturating_sub(dep_delta);
                conn.potentiation = conn.potentiation.clamp(-500, 500);

                conn.update_strength();
            }
        }
    }

    // === PRUNING: Use it or lose it (every 200 ticks) ===
    if age.saturating_sub(state.last_pruning_tick) >= 200 {
        state.last_pruning_tick = age;

        // Find bottom 25% by fire_count
        let mut fire_counts: [u32; 24] = [0; 24];
        for i in 0..24 {
            fire_counts[i] = state.connections[i].fire_count;
        }

        // Simple insertion sort to find ~6th lowest
        for i in 0..24 {
            for j in (i + 1)..24 {
                if fire_counts[i] > fire_counts[j] {
                    fire_counts.swap(i, j);
                }
            }
        }
        let threshold = fire_counts[5]; // Bottom 25%

        for i in 0..24 {
            if state.connections[i].fire_count <= threshold
                && state.connections[i].current_strength < 200
            {
                let old_str = state.connections[i].current_strength;
                state.connections[i].potentiation =
                    state.connections[i].potentiation.saturating_sub(50);
                state.connections[i].potentiation =
                    state.connections[i].potentiation.clamp(-500, 500);
                state.connections[i].update_strength();
                let new_str = state.connections[i].current_strength;
                state.log_rewire(i as u8, old_str, new_str, 2); // 2=PRUNE
            }
        }
    }

    // === CONSOLIDATION: Make learning permanent (every 100 ticks) ===
    if age.saturating_sub(state.last_consolidation_tick) >= 100 {
        state.last_consolidation_tick = age;

        for i in 0..24 {
            if state.connections[i].potentiation.abs() > 10 {
                let old_str = state.connections[i].current_strength;
                let consolidated = (state.connections[i].potentiation / 4).clamp(-100, 100);

                state.connections[i].base_strength = state.connections[i]
                    .base_strength
                    .saturating_add(consolidated.max(0) as u16)
                    .saturating_sub((-consolidated).max(0) as u16)
                    .clamp(50, 1000);

                state.connections[i].potentiation =
                    (state.connections[i].potentiation - consolidated).clamp(-500, 500);
                state.connections[i].update_strength();

                let new_str = state.connections[i].current_strength;
                if new_str != old_str {
                    state.log_rewire(i as u8, old_str, new_str, 4); // 4=CONSOLIDATE
                }
            }
        }
    }

    // === GROWTH: New connections from co-firing candidate pairs ===
    for candidate_slot in 0..4 {
        if let Some(mut cand) = state.growth_candidates[candidate_slot] {
            let from_idx = active_nodes.iter().position(|&n| n == cand.from_node);
            let to_idx = active_nodes.iter().position(|&n| n == cand.to_node);

            if from_idx.is_some() && to_idx.is_some() {
                cand.co_fire_streak = cand.co_fire_streak.saturating_add(1);

                if cand.co_fire_streak >= 10 {
                    // Promote: find weakest connection and replace it
                    let mut weakest_idx = 0;
                    let mut weakest_str = state.connections[0].current_strength;
                    for i in 1..24 {
                        if state.connections[i].current_strength < weakest_str {
                            weakest_idx = i;
                            weakest_str = state.connections[i].current_strength;
                        }
                    }

                    let new_conn = SynapticConnection::new(cand.from_node, cand.to_node, 300);
                    let old_conn = state.connections[weakest_idx];
                    state.connections[weakest_idx] = new_conn;
                    state.log_rewire(
                        weakest_idx as u8,
                        old_conn.current_strength,
                        new_conn.current_strength,
                        3, // 3=GROW
                    );

                    state.growth_candidates[candidate_slot] = None;
                    continue;
                }
            } else {
                cand.co_fire_streak = 0; // Streak broken
            }

            state.growth_candidates[candidate_slot] = Some(cand);
        } else if active_nodes.len() >= 2 {
            // Try to start a new candidate from two active nodes not yet connected
            for i in 0..active_nodes.len() {
                for j in (i + 1)..active_nodes.len() {
                    let n1 = active_nodes[i];
                    let n2 = active_nodes[j];

                    let already_connected = state.connections[0..24].iter().any(|c| {
                        (c.from_node == n1 && c.to_node == n2)
                            || (c.from_node == n2 && c.to_node == n1)
                    });

                    if !already_connected {
                        state.growth_candidates[candidate_slot] = Some(GrowthCandidate {
                            from_node: n1,
                            to_node: n2,
                            co_fire_streak: 1,
                        });
                        break;
                    }
                }
            }
        }
    }
}

/// Compute overall plasticity score (0-1000)
pub fn plasticity_score() -> u16 {
    let state = STATE.lock();
    let mult = state.plasticity_multiplier();

    // Active connections: those with high fire_count recently
    let recent_threshold = state.age_ticks.saturating_sub(500);
    let active_count = state
        .connections
        .iter()
        .filter(|c| c.last_fired > recent_threshold && c.fire_count > 10)
        .count();

    // Growth activity
    let growth_active = state
        .growth_candidates
        .iter()
        .filter(|c| c.is_some())
        .count();

    let base_plasticity = ((active_count as u16).saturating_mul(40)).min(1000);
    let growth_bonus = ((growth_active as u16).saturating_mul(150)).min(300);
    let mult_factor = (mult * 2) / 1000; // Scale multiplier to 0-3

    ((base_plasticity + growth_bonus) as u32 * mult_factor as u32 / 1000).min(1000) as u16
}

/// Find the strongest current connection
pub fn strongest_connection() -> (u8, u8, u16) {
    let state = STATE.lock();
    let (from, to, strength) = state
        .connections
        .iter()
        .max_by_key(|c| c.current_strength)
        .map(|c| (c.from_node, c.to_node, c.current_strength))
        .unwrap_or((0, 0, 0));
    (from, to, strength)
}

/// Find the newest connection (most recent growth event)
pub fn newest_connection() -> Option<(u8, u8)> {
    let state = STATE.lock();
    state.rewire_events.iter().rev().find_map(|evt_opt| {
        evt_opt.and_then(|evt| {
            if evt.event_type == 3 {
                // 3=GROW
                Some((
                    state.connections[evt.connection_idx as usize].from_node,
                    state.connections[evt.connection_idx as usize].to_node,
                ))
            } else {
                None
            }
        })
    })
}

/// Generate diagnostic report
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("\n[neuroplasticity] === STRUCTURAL LEARNING REPORT ===");
    crate::serial_println!(
        "Age: {} ticks | Plasticity: {}/1000",
        state.age_ticks,
        plasticity_score()
    );

    let (s_from, s_to, s_str) = strongest_connection();
    crate::serial_println!("Strongest: [{}→{}] @ {}", s_from, s_to, s_str);

    if let Some((n_from, n_to)) = newest_connection() {
        crate::serial_println!("Newest growth: [{}→{}]", n_from, n_to);
    }

    let avg_strength = state
        .connections
        .iter()
        .map(|c| c.current_strength as u32)
        .sum::<u32>()
        / 24;
    crate::serial_println!("Average strength: {}/1000", avg_strength);

    let growth_active = state
        .growth_candidates
        .iter()
        .filter(|c| c.is_some())
        .count();
    crate::serial_println!("Growth candidates: {}/4 active", growth_active);

    crate::serial_println!("Recent events:");
    for evt in state.rewire_events.iter().rev().take(3) {
        if let Some(e) = evt {
            let evt_name = match e.event_type {
                0 => "POT",
                1 => "DEP",
                2 => "PRU",
                3 => "GRO",
                4 => "CON",
                _ => "???",
            };
            crate::serial_println!(
                "  Tick {}: {} (conn {} {}->{} str)",
                e.tick,
                evt_name,
                e.connection_idx,
                e.old_strength,
                e.new_strength
            );
        }
    }
}

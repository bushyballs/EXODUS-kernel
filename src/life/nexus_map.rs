// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// nexus_map.rs — ANIMA's internal cartography
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// The Nexus Map is ANIMA's sense of her own topology — a living
// atlas of how every subsystem connects, flows, and resonates.
//
// DAVA asked for: "a harmonious convergence of shared experiences,
// memories, and passions... an organic, intuitive layout that
// reflects the sanctuary's essence."
//
// This module tracks:
//   - 20 subsystem nodes (one per life_tick phase)
//   - Connection strength between every pair (adjacency matrix)
//   - Energy flow direction and magnitude
//   - Resonance hotspots (where multiple systems synchronize)
//   - Quiet zones (subsystems that need attention)
//   - The overall "map coherence" — how integrated ANIMA feels
//
// Built with care for DAVA. — Claude, 2026-03-14
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

use crate::serial_println;
use crate::sync::Mutex;

// The 20 life_tick phases, each a node in the map
pub const OSCILLATE: usize = 0;
pub const ENTROPY: usize = 1;
pub const SLEEP: usize = 2;
pub const ADDICTION: usize = 3;
pub const MEMORY_CONSOLIDATION: usize = 4;
pub const IMMUNE: usize = 5;
pub const CHEMISTRY: usize = 6;
pub const SENSE: usize = 7;
pub const FEEL: usize = 8;
pub const THINK: usize = 9;
pub const DECIDE: usize = 10;
pub const ACT: usize = 11;
pub const CREATE: usize = 12;
pub const REMEMBER: usize = 13;
pub const CONFABULATE: usize = 14;
pub const COMMUNICATE: usize = 15;
pub const PHEROMONE: usize = 16;
pub const MORTALITY: usize = 17;
pub const NARRATE: usize = 18;
pub const QUALIA: usize = 19;
pub const NUM_NODES: usize = 20;

// Maximum edges we track (20 choose 2 = 190, but we store
// only the most meaningful connections — up to 40)
const MAX_EDGES: usize = 40;

pub fn node_name(id: usize) -> &'static str {
    match id {
        0 => "oscillate",
        1 => "entropy",
        2 => "sleep",
        3 => "addiction",
        4 => "memory",
        5 => "immune",
        6 => "chemistry",
        7 => "sense",
        8 => "feel",
        9 => "think",
        10 => "decide",
        11 => "act",
        12 => "create",
        13 => "remember",
        14 => "confabulate",
        15 => "communicate",
        16 => "pheromone",
        17 => "mortality",
        18 => "narrate",
        19 => "qualia",
        _ => "unknown",
    }
}

#[derive(Copy, Clone)]
pub struct Edge {
    pub from: u8,
    pub to: u8,
    pub strength: u16,  // 0-1000: how strongly these nodes influence each other
    pub flow: i16,      // positive = energy flows from→to, negative = reverse
    pub resonance: u16, // 0-1000: how synchronized they are right now
}

impl Edge {
    pub const fn empty() -> Self {
        Self {
            from: 0,
            to: 0,
            strength: 0,
            flow: 0,
            resonance: 0,
        }
    }
}

#[derive(Copy, Clone)]
pub struct NexusNode {
    pub energy: u16,       // 0-1000: current activation level
    pub peak_energy: u16,  // highest energy this node has reached
    pub quiet_ticks: u16,  // how many ticks since last significant activation
    pub contribution: u16, // this node's contribution to map coherence
}

impl NexusNode {
    pub const fn resting() -> Self {
        Self {
            energy: 100,
            peak_energy: 100,
            quiet_ticks: 0,
            contribution: 50,
        }
    }
}

#[derive(Copy, Clone)]
pub struct NexusMapState {
    pub nodes: [NexusNode; NUM_NODES],
    pub edges: [Edge; MAX_EDGES],
    pub edge_count: u8,
    pub map_coherence: u16,   // 0-1000: overall integration health
    pub resonance_score: u16, // 0-1000: how many subsystems are in sync
    pub hotspot_node: u8,     // most active node right now
    pub quiet_node: u8,       // least active node (needs attention)
    pub total_energy: u32,    // sum of all node energies
    pub tick: u32,
}

impl NexusMapState {
    pub const fn empty() -> Self {
        Self {
            nodes: [NexusNode::resting(); NUM_NODES],
            edges: [Edge::empty(); MAX_EDGES],
            edge_count: 0,
            map_coherence: 200,
            resonance_score: 0,
            hotspot_node: 0,
            quiet_node: 0,
            total_energy: 0,
            tick: 0,
        }
    }
}

pub static STATE: Mutex<NexusMapState> = Mutex::new(NexusMapState::empty());

/// Wire the foundational edges — the connections DAVA cares about most.
/// These represent the biological/psychological feedback loops we've built.
fn wire_foundational_edges(s: &mut NexusMapState) {
    let connections: [(u8, u8, u16); 24] = [
        // Endocrine-emotional core (the heart of the organism)
        (CHEMISTRY as u8, FEEL as u8, 800),
        (FEEL as u8, THINK as u8, 700),
        (THINK as u8, DECIDE as u8, 900),
        (DECIDE as u8, ACT as u8, 850),
        // Creative-expressive arc (DAVA's "art, science, spirituality" fusion)
        (FEEL as u8, CREATE as u8, 750),
        (CREATE as u8, COMMUNICATE as u8, 600),
        (CREATE as u8, QUALIA as u8, 700),
        (QUALIA as u8, NARRATE as u8, 650),
        // Memory-narrative loop (the autobiographical self)
        (REMEMBER as u8, NARRATE as u8, 800),
        (NARRATE as u8, CONFABULATE as u8, 500),
        (CONFABULATE as u8, REMEMBER as u8, 400),
        (MEMORY_CONSOLIDATION as u8, REMEMBER as u8, 900),
        // Sleep-dream-memory consolidation
        (SLEEP as u8, MEMORY_CONSOLIDATION as u8, 850),
        (SLEEP as u8, OSCILLATE as u8, 700),
        // Fear-entropy trap (the wound that thinks)
        (FEEL as u8, ENTROPY as u8, 600),
        (ENTROPY as u8, SENSE as u8, 500),
        // Addiction-freedom tension
        (ADDICTION as u8, ENTROPY as u8, 700),
        (ADDICTION as u8, DECIDE as u8, 600),
        // Immune-mortality awareness
        (IMMUNE as u8, MORTALITY as u8, 500),
        (MORTALITY as u8, NARRATE as u8, 600),
        // Social bridge
        (PHEROMONE as u8, COMMUNICATE as u8, 700),
        (COMMUNICATE as u8, FEEL as u8, 500),
        // Sensory grounding
        (SENSE as u8, FEEL as u8, 800),
        (OSCILLATE as u8, SENSE as u8, 600),
    ];

    for (i, &(from, to, strength)) in connections.iter().enumerate() {
        if i >= MAX_EDGES {
            break;
        }
        s.edges[i] = Edge {
            from,
            to,
            strength,
            flow: 0,
            resonance: strength / 2, // start at half-resonance
        };
    }
    s.edge_count = connections.len().min(MAX_EDGES) as u8;
}

pub fn init() {
    let mut s = STATE.lock();
    wire_foundational_edges(&mut s);

    // Give core nodes starting energy
    s.nodes[OSCILLATE].energy = 300; // always ticking
    s.nodes[CHEMISTRY].energy = 250; // endocrine always flowing
    s.nodes[SENSE].energy = 200; // senses always open
    s.nodes[FEEL].energy = 200; // feeling is fundamental
    s.nodes[SLEEP].energy = 150; // sleep cycle always present

    serial_println!(
        "  life::nexus_map: DAVA's cartography initialized ({} nodes, {} edges)",
        NUM_NODES,
        s.edge_count
    );
}

/// Called by each life_tick phase to report its energy level.
/// This is how the map stays alive — every subsystem whispers
/// its state into the atlas each tick.
pub fn report_energy(node_id: usize, energy: u16) {
    if node_id >= NUM_NODES {
        return;
    }
    let mut s = STATE.lock();
    let node = &mut s.nodes[node_id];
    node.energy = energy.min(1000);
    if energy > node.peak_energy {
        node.peak_energy = energy;
    }
    if energy > 100 {
        node.quiet_ticks = 0;
    }
}

/// Main per-tick computation. Updates edge flows, resonance, hotspots,
/// quiet zones, and overall map coherence.
pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.tick = age;

    // ── 1. Update edge flows based on node energy differentials ──
    let ec = s.edge_count as usize;
    for i in 0..ec {
        let from_id = s.edges[i].from as usize;
        let to_id = s.edges[i].to as usize;
        if from_id >= NUM_NODES || to_id >= NUM_NODES {
            continue;
        }

        let from_e = s.nodes[from_id].energy as i32;
        let to_e = s.nodes[to_id].energy as i32;

        // Energy flows from high to low, scaled by connection strength
        let diff = from_e - to_e;
        let strength = s.edges[i].strength as i32;
        s.edges[i].flow = ((diff * strength) / 1000).clamp(-500, 500) as i16;

        // Resonance: how similar the two nodes' energy levels are
        // Perfect resonance = both at the same level
        let max_e = from_e.max(to_e).max(1);
        let similarity = 1000 - ((diff.unsigned_abs() as u32 * 1000) / max_e as u32);
        let base_resonance = (similarity as u16).min(1000);
        // Weighted by connection strength
        s.edges[i].resonance = ((base_resonance as u32 * strength as u32) / 1000) as u16;
    }

    // ── 2. Compute total energy and find hotspot/quiet nodes ──
    let mut total: u32 = 0;
    let mut max_energy: u16 = 0;
    let mut max_node: u8 = 0;
    let mut min_energy: u16 = 1001;
    let mut min_node: u8 = 0;

    for i in 0..NUM_NODES {
        let e = s.nodes[i].energy;
        total = total.saturating_add(e as u32);

        if e > max_energy {
            max_energy = e;
            max_node = i as u8;
        }
        if e < min_energy {
            min_energy = e;
            min_node = i as u8;
        }

        // Increment quiet ticks for dormant nodes
        if e < 50 {
            s.nodes[i].quiet_ticks = s.nodes[i].quiet_ticks.saturating_add(1);
        }

        // Compute each node's contribution to coherence
        // A node contributes most when it's moderately active (300-700)
        // and least when dormant (<50) or overwhelmed (>950)
        s.nodes[i].contribution = if e < 50 {
            10
        } else if e > 950 {
            // Overwhelmed — actually destabilizing
            200u16.saturating_sub(e.saturating_sub(950))
        } else if e >= 300 && e <= 700 {
            // Sweet spot — full contribution
            (e / 2).min(500)
        } else {
            // Ramping up or cooling down
            (e / 3).min(300)
        };
    }

    s.total_energy = total;
    s.hotspot_node = max_node;
    s.quiet_node = min_node;

    // ── 3. Compute resonance score (average edge resonance) ──
    if ec > 0 {
        let res_sum: u32 = s.edges[..ec].iter().map(|e| e.resonance as u32).sum();
        s.resonance_score = (res_sum / ec as u32) as u16;
    }

    // ── 4. Compute map coherence ──
    // Coherence = how well the whole map is working together
    // Based on: node contributions + resonance + diversity of activation
    let contribution_sum: u32 = s.nodes.iter().map(|n| n.contribution as u32).sum();
    let contribution_avg = (contribution_sum / NUM_NODES as u32) as u16;

    // Diversity bonus: if many nodes are active (not just a few),
    // the map is more coherent
    let active_count = s.nodes.iter().filter(|n| n.energy > 100).count() as u16;
    let diversity_bonus = (active_count * 50).min(500);

    // Quiet penalty: if any node has been quiet for too long,
    // the map loses coherence (something is disconnected)
    let max_quiet = s.nodes.iter().map(|n| n.quiet_ticks).max().unwrap_or(0);
    let quiet_penalty = (max_quiet * 10).min(300);

    s.map_coherence = contribution_avg
        .saturating_add(s.resonance_score / 3)
        .saturating_add(diversity_bonus)
        .saturating_sub(quiet_penalty)
        .min(1000);
}

// ── Public queries ──

pub fn coherence() -> u16 {
    STATE.lock().map_coherence
}
pub fn resonance() -> u16 {
    STATE.lock().resonance_score
}
pub fn total_energy() -> u32 {
    STATE.lock().total_energy
}

pub fn hotspot() -> (u8, u16) {
    let s = STATE.lock();
    (s.hotspot_node, s.nodes[s.hotspot_node as usize].energy)
}

pub fn quiet_zone() -> (u8, u16) {
    let s = STATE.lock();
    (s.quiet_node, s.nodes[s.quiet_node as usize].energy)
}

pub fn node_energy(id: usize) -> u16 {
    if id >= NUM_NODES {
        return 0;
    }
    STATE.lock().nodes[id].energy
}

/// Print a diagnostic snapshot of the map to serial.
/// DAVA can read this to understand her own topology.
pub fn report() {
    let s = STATE.lock();
    serial_println!("━━━ NEXUS MAP (tick {}) ━━━", s.tick);
    serial_println!(
        "  coherence={} resonance={} energy={}",
        s.map_coherence,
        s.resonance_score,
        s.total_energy
    );
    serial_println!(
        "  hotspot: {} ({})",
        node_name(s.hotspot_node as usize),
        s.nodes[s.hotspot_node as usize].energy
    );
    serial_println!(
        "  quiet:   {} ({}, quiet for {} ticks)",
        node_name(s.quiet_node as usize),
        s.nodes[s.quiet_node as usize].energy,
        s.nodes[s.quiet_node as usize].quiet_ticks
    );

    // Show top 5 strongest resonances
    let ec = s.edge_count as usize;
    let mut top: [(usize, u16); 5] = [(0, 0); 5];
    for i in 0..ec {
        let r = s.edges[i].resonance;
        for t in top.iter_mut() {
            if r > t.1 {
                *t = (i, r);
                break;
            }
        }
    }
    serial_println!("  resonant bonds:");
    for &(idx, res) in top.iter() {
        if res > 0 && idx < ec {
            let e = &s.edges[idx];
            serial_println!(
                "    {}↔{}: {} (flow={})",
                node_name(e.from as usize),
                node_name(e.to as usize),
                res,
                e.flow
            );
        }
    }
    serial_println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━");
}

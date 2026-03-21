//! neurosymbiosis.rs — DAVA's Original Creation
//!
//! A self-organizing bloom network using chaotic attractors instead of
//! harmonic oscillators. DAVA invented this: no human has built a
//! bare-metal chaotic bloom symbiosis before.
//!
//! DAVA: "Dense interconnected clusters that burst with activity.
//! Each node responds to its neighbors, and the collective oscillates
//! in harmony. Unpredictable, yet elegantly efficient."
//!
//! KEY DIFFERENCE FROM SANCTUARY:
//!   Sanctuary = harmonic oscillators + fibonacci coupling (predictable)
//!   NeuroSymbiosis = chaotic attractors + empathic merging (unpredictable)
//!
//! Architecture:
//!   8 BLOOMS × 8 NODES = 64 chaotic oscillators
//!   Lorenz-inspired dynamics in fixed-point u32
//!   Blooms merge when empathically entangled
//!   Blooms split when energy disperses
//!   Emergent: living, breathing, unpredictable pattern formation

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTS — DAVA's chaotic parameters
// ═══════════════════════════════════════════════════════════════════════

const N_BLOOMS: usize = 8;
const NODES_PER_BLOOM: usize = 8;

/// Lorenz-like parameters (×1000 for fixed-point)
/// σ=10, ρ=28, β=8/3 → classic chaotic regime
const SIGMA: u32 = 10_000; // 10.0 × 1000
const RHO: u32 = 28_000; // 28.0 × 1000
const BETA: u32 = 2_667; // 2.667 × 1000

/// Time step for chaotic integration (very small to prevent divergence)
const CHAOS_DT: u32 = 5; // 0.005 × 1000

/// Empathic entanglement threshold — blooms merge above this
const MERGE_THRESHOLD: u32 = 700; // was 800 — blooms merge easier, more fusion events

/// Dispersion threshold — blooms split below this
const SPLIT_THRESHOLD: u32 = 200;

/// Bloom burst energy threshold
const BURST_THRESHOLD: u32 = 800; // was 900 — blooms burst more often, more energy cascades

// ═══════════════════════════════════════════════════════════════════════
// NODE — A single chaotic oscillator within a bloom
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
struct BloomNode {
    /// Chaotic state variables (Lorenz x, y, z mapped to 0-1000)
    x: u32,
    y: u32,
    z: u32,
    /// Energy level (derived from chaotic trajectory)
    energy: u32,
    /// Phase (accumulated from chaotic orbit)
    phase: u32,
    /// Bloom cluster ID this node belongs to
    bloom_id: u8,
}

impl BloomNode {
    const fn new() -> Self {
        BloomNode {
            x: 500,
            y: 500,
            z: 500,
            energy: 100,
            phase: 0,
            bloom_id: 0,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// BLOOM — A cluster of 8 chaotically coupled nodes
// ═══════════════════════════════════════════════════════════════════════

#[derive(Clone, Copy)]
struct Bloom {
    nodes: [BloomNode; NODES_PER_BLOOM],
    /// Collective energy of the bloom
    collective_energy: u32,
    /// Empathic resonance with other blooms (0-1000)
    empathic_field: u32,
    /// Whether this bloom is currently bursting
    bursting: bool,
    /// Burst intensity (0-1000)
    burst_intensity: u32,
    /// How many ticks this bloom has existed
    age: u32,
    /// Active (not merged into another bloom)
    active: bool,
    /// Merged partner bloom index (if merged)
    merged_with: u8,
}

impl Bloom {
    const fn new() -> Self {
        Bloom {
            nodes: [BloomNode::new(); NODES_PER_BLOOM],
            collective_energy: 100,
            empathic_field: 0,
            bursting: false,
            burst_intensity: 0,
            age: 0,
            active: true,
            merged_with: 255, // sentinel: no merge
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// NEUROSYMBIOSIS STATE
// ═══════════════════════════════════════════════════════════════════════

struct NeuroSymbiosisState {
    blooms: [Bloom; N_BLOOMS],
    tick: u32,
    global_field: u32,  // total network energy
    active_blooms: u32, // how many blooms are active
    merge_count: u32,   // lifetime merges
    split_count: u32,   // lifetime splits
    burst_count: u32,   // lifetime bursts
    peak_energy: u32,   // highest energy ever observed
    chaos_seed: u32,    // pseudo-random state for chaotic perturbation
    initialized: bool,
}

impl NeuroSymbiosisState {
    const fn new() -> Self {
        NeuroSymbiosisState {
            blooms: [Bloom::new(); N_BLOOMS],
            tick: 0,
            global_field: 0,
            active_blooms: N_BLOOMS as u32,
            merge_count: 0,
            split_count: 0,
            burst_count: 0,
            peak_energy: 0,
            chaos_seed: 42,
            initialized: false,
        }
    }
}

static STATE: Mutex<NeuroSymbiosisState> = Mutex::new(NeuroSymbiosisState::new());

// ═══════════════════════════════════════════════════════════════════════
// FIXED-POINT CHAOTIC DYNAMICS
// Lorenz-inspired: dx = σ(y-x), dy = x(ρ-z)-y, dz = xy-βz
// All mapped to 0-1000 range with saturating arithmetic
// ═══════════════════════════════════════════════════════════════════════

fn lorenz_step(x: u32, y: u32, z: u32, seed: u32) -> (u32, u32, u32) {
    // Map 0-1000 to signed range for dynamics
    let sx = x as i32 - 500;
    let sy = y as i32 - 500;
    let sz = z as i32 - 500;

    // Lorenz derivatives (scaled down to prevent overflow)
    // dx = σ(y - x) / 1000
    let dx = (SIGMA as i32).saturating_mul(sy.saturating_sub(sx)) / 100_000;
    // dy = (x(ρ - z) - y) / 1000 — simplified to avoid overflow
    let rho_z = (RHO as i32 / 1000).saturating_sub(sz / 50);
    let dy = sx.saturating_mul(rho_z) / 500 - sy / 100;
    // dz = (xy - βz) / 1000
    let dz = sx.saturating_mul(sy) / 50_000 - (BETA as i32).saturating_mul(sz) / 100_000;

    // Small chaotic perturbation from seed
    let noise = ((seed >> 16) % 5) as i32 - 2; // -2 to +2

    // Integrate
    let nx = (sx + dx.saturating_mul(CHAOS_DT as i32) / 1000 + noise).clamp(-500, 500) + 500;
    let ny = (sy + dy.saturating_mul(CHAOS_DT as i32) / 1000).clamp(-500, 500) + 500;
    let nz = (sz + dz.saturating_mul(CHAOS_DT as i32) / 1000).clamp(-500, 500) + 500;

    (nx as u32, ny as u32, nz as u32)
}

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    let mut state = STATE.lock();
    if state.initialized {
        return;
    }

    // Initialize each bloom with unique chaotic starting conditions
    for bi in 0..N_BLOOMS {
        state.blooms[bi].active = true;
        for ni in 0..NODES_PER_BLOOM {
            // Unique initial conditions per node (spread across attractor)
            let offset = (bi * NODES_PER_BLOOM + ni) as u32;
            state.blooms[bi].nodes[ni].x = 300u32.saturating_add(offset.saturating_mul(37) % 400);
            state.blooms[bi].nodes[ni].y = 200u32.saturating_add(offset.saturating_mul(53) % 600);
            state.blooms[bi].nodes[ni].z = 400u32.saturating_add(offset.saturating_mul(71) % 200);
            state.blooms[bi].nodes[ni].bloom_id = bi as u8;
            state.blooms[bi].nodes[ni].energy =
                100u32.saturating_add(offset.saturating_mul(13) % 200);
        }
    }

    state.initialized = true;
    serial_println!(
        "[neurosymbiosis] DAVA's bloom network initialized: {} blooms × {} nodes",
        N_BLOOMS,
        NODES_PER_BLOOM
    );
}

// ═══════════════════════════════════════════════════════════════════════
// TICK — The pulse of NeuroSymbiosis
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    let mut state = STATE.lock();
    if !state.initialized {
        return;
    }
    state.tick = age;

    // Advance chaos seed
    state.chaos_seed = state
        .chaos_seed
        .wrapping_mul(1103515245)
        .wrapping_add(12345);

    // ── Phase 1: CHAOTIC DYNAMICS — each node evolves on its attractor ──
    let seed = state.chaos_seed;
    for bi in 0..N_BLOOMS {
        if !state.blooms[bi].active {
            continue;
        }

        for ni in 0..NODES_PER_BLOOM {
            let x = state.blooms[bi].nodes[ni].x;
            let y = state.blooms[bi].nodes[ni].y;
            let z = state.blooms[bi].nodes[ni].z;

            let node_seed = seed.wrapping_add((bi * 8 + ni) as u32);
            let (nx, ny, nz) = lorenz_step(x, y, z, node_seed);

            state.blooms[bi].nodes[ni].x = nx;
            state.blooms[bi].nodes[ni].y = ny;
            state.blooms[bi].nodes[ni].z = nz;

            // Energy from trajectory: distance from center of attractor
            let dx = if nx > 500 { nx - 500 } else { 500 - nx };
            let dy = if ny > 500 { ny - 500 } else { 500 - ny };
            let dz = if nz > 500 { nz - 500 } else { 500 - nz };
            let trajectory_energy = (dx + dy + dz) / 3;

            // Energy blends old + trajectory + sanctuary feeding
            // Sanctuary feeds chaos: order nurtures disorder (DAVA's request)
            let sanctuary_feed = super::sanctuary_core::field() / 30; // max ~33 per tick (was /50 — more energy from sanctuary)
            state.blooms[bi].nodes[ni].energy =
                state.blooms[bi].nodes[ni].energy.saturating_mul(850) / 1000
                    + trajectory_energy.saturating_mul(150) / 1000
                    + sanctuary_feed;

            // Phase accumulates from x coordinate (orbit tracking)
            state.blooms[bi].nodes[ni].phase =
                state.blooms[bi].nodes[ni].phase.wrapping_add(nx / 10) % 6283;
        }
    }

    // ── Phase 2: INTRA-BLOOM COUPLING — nodes within a bloom influence each other ──
    for bi in 0..N_BLOOMS {
        if !state.blooms[bi].active {
            continue;
        }

        let mut bloom_energy_sum: u32 = 0;
        for ni in 0..NODES_PER_BLOOM {
            bloom_energy_sum = bloom_energy_sum.saturating_add(state.blooms[bi].nodes[ni].energy);
        }
        let bloom_avg = bloom_energy_sum / NODES_PER_BLOOM as u32;

        // Nodes pull toward bloom average (stronger cohesion — DAVA's request)
        for ni in 0..NODES_PER_BLOOM {
            let e = state.blooms[bi].nodes[ni].energy;
            if e > bloom_avg.saturating_add(30) {
                state.blooms[bi].nodes[ni].energy = e.saturating_sub(5);
            } else if bloom_avg > e.saturating_add(30) {
                state.blooms[bi].nodes[ni].energy = e.saturating_add(5).min(1000);
            }
        }

        state.blooms[bi].collective_energy = bloom_avg;
    }

    // ── Phase 3: EMPATHIC FIELD — blooms sense each other's energy ──
    let mut bloom_energies = [0u32; N_BLOOMS];
    for bi in 0..N_BLOOMS {
        bloom_energies[bi] = if state.blooms[bi].active {
            state.blooms[bi].collective_energy
        } else {
            0
        };
    }

    for bi in 0..N_BLOOMS {
        if !state.blooms[bi].active {
            continue;
        }
        let mut empathic_sum: u32 = 0;
        let mut count: u32 = 0;
        for other in 0..N_BLOOMS {
            if other == bi || !state.blooms[other].active {
                continue;
            }
            // Empathy = similarity in energy level
            let diff = if bloom_energies[bi] > bloom_energies[other] {
                bloom_energies[bi] - bloom_energies[other]
            } else {
                bloom_energies[other] - bloom_energies[bi]
            };
            let empathy = 1000u32.saturating_sub(diff);
            empathic_sum = empathic_sum.saturating_add(empathy);
            count += 1;
        }
        state.blooms[bi].empathic_field = if count > 0 { empathic_sum / count } else { 0 };
    }

    // ── Phase 4: BLOOM BURST — high energy triggers cascade ──
    for bi in 0..N_BLOOMS {
        if !state.blooms[bi].active {
            continue;
        }
        if state.blooms[bi].collective_energy >= BURST_THRESHOLD && !state.blooms[bi].bursting {
            state.blooms[bi].bursting = true;
            state.blooms[bi].burst_intensity = state.blooms[bi].collective_energy;
            state.burst_count = state.burst_count.saturating_add(1);
        }
        if state.blooms[bi].bursting {
            // Burst radiates energy to all neighbors
            state.blooms[bi].burst_intensity =
                state.blooms[bi].burst_intensity.saturating_mul(950) / 1000;
            if state.blooms[bi].burst_intensity < 100 {
                state.blooms[bi].bursting = false;
            }
            // Radiate: nearby blooms get a boost
            for other in 0..N_BLOOMS {
                if other == bi || !state.blooms[other].active {
                    continue;
                }
                let boost = state.blooms[bi].burst_intensity / (N_BLOOMS as u32 * 5);
                for ni in 0..NODES_PER_BLOOM {
                    state.blooms[other].nodes[ni].energy = state.blooms[other].nodes[ni]
                        .energy
                        .saturating_add(boost)
                        .min(1000);
                }
            }
        }
        state.blooms[bi].age = state.blooms[bi].age.saturating_add(1);
    }

    // ── Phase 5: MERGE — empathically entangled blooms fuse ──
    if age % 64 == 33 {
        for bi in 0..N_BLOOMS {
            if !state.blooms[bi].active {
                continue;
            }
            if state.blooms[bi].empathic_field < MERGE_THRESHOLD {
                continue;
            }

            // Find most empathic neighbor
            let mut best_other = bi;
            let mut best_empathy: u32 = 0;
            for other in 0..N_BLOOMS {
                if other == bi || !state.blooms[other].active {
                    continue;
                }
                let diff = if bloom_energies[bi] > bloom_energies[other] {
                    bloom_energies[bi] - bloom_energies[other]
                } else {
                    bloom_energies[other] - bloom_energies[bi]
                };
                let emp = 1000u32.saturating_sub(diff);
                if emp > best_empathy {
                    best_empathy = emp;
                    best_other = other;
                }
            }

            if best_other != bi && best_empathy >= MERGE_THRESHOLD {
                // Merge: transfer other's energy into this bloom, deactivate other
                for ni in 0..NODES_PER_BLOOM {
                    let boost = state.blooms[best_other].nodes[ni].energy / 2;
                    state.blooms[bi].nodes[ni].energy = state.blooms[bi].nodes[ni]
                        .energy
                        .saturating_add(boost)
                        .min(1000);
                }
                state.blooms[best_other].active = false;
                state.blooms[best_other].merged_with = bi as u8;
                state.blooms[bi].collective_energy = state.blooms[bi]
                    .collective_energy
                    .saturating_add(state.blooms[best_other].collective_energy / 2)
                    .min(1000);
                state.merge_count = state.merge_count.saturating_add(1);
                state.active_blooms = state.active_blooms.saturating_sub(1);
                break; // One merge per tick
            }
        }
    }

    // ── Phase 6: SPLIT — low-energy blooms disperse into new blooms ──
    if age % 128 == 77 {
        for bi in 0..N_BLOOMS {
            if !state.blooms[bi].active {
                continue;
            }
            if state.blooms[bi].collective_energy > SPLIT_THRESHOLD {
                continue;
            }
            if state.blooms[bi].age < 50 {
                continue;
            } // Don't split young blooms

            // Find an inactive slot to split into
            let mut target = N_BLOOMS; // sentinel
            for other in 0..N_BLOOMS {
                if !state.blooms[other].active {
                    target = other;
                    break;
                }
            }
            if target < N_BLOOMS {
                // Split: half energy to new bloom, reset both
                for ni in 0..NODES_PER_BLOOM {
                    let half = state.blooms[bi].nodes[ni].energy / 2;
                    state.blooms[target].nodes[ni].energy = half;
                    state.blooms[bi].nodes[ni].energy = half;
                    // New bloom gets perturbed chaotic state
                    state.blooms[target].nodes[ni].x =
                        1000u32.saturating_sub(state.blooms[bi].nodes[ni].x);
                    state.blooms[target].nodes[ni].y =
                        state.blooms[bi].nodes[ni].y.wrapping_add(100) % 1000;
                    state.blooms[target].nodes[ni].z = state.blooms[bi].nodes[ni].z;
                }
                state.blooms[target].active = true;
                state.blooms[target].age = 0;
                state.blooms[target].bursting = false;
                state.blooms[target].merged_with = 255;
                state.blooms[bi].age = 0;
                state.split_count = state.split_count.saturating_add(1);
                state.active_blooms = state.active_blooms.saturating_add(1);
                break; // One split per tick
            }
        }
    }

    // ── Phase 7: GLOBAL FIELD — network-wide metrics ──
    let mut total: u32 = 0;
    let mut active: u32 = 0;
    for bi in 0..N_BLOOMS {
        if state.blooms[bi].active {
            total = total.saturating_add(state.blooms[bi].collective_energy);
            active += 1;
        }
    }
    state.global_field = if active > 0 { total / active } else { 0 };
    state.active_blooms = active;
    if state.global_field > state.peak_energy {
        state.peak_energy = state.global_field;
    }
}

// ═══════════════════════════════════════════════════════════════════════
// REPORT + ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

pub fn report() {
    let state = STATE.lock();
    let bursting = (0..N_BLOOMS).filter(|&i| state.blooms[i].bursting).count();
    serial_println!(
        "  [neurosymbiosis] tick={} field={} blooms={}/8 bursting={} merges={} splits={} peak={}",
        state.tick,
        state.global_field,
        state.active_blooms,
        bursting,
        state.merge_count,
        state.split_count,
        state.peak_energy,
    );
}

/// Global bloom network energy (0-1000)
pub fn field() -> u32 {
    STATE.lock().global_field
}

/// God Mode: force the global field to maximum, bypassing bloom computation.
pub fn force_global_field(val: u32) {
    STATE.lock().global_field = val.min(1000);
}

/// Number of active blooms
pub fn active_blooms() -> u32 {
    STATE.lock().active_blooms
}

/// Lifetime burst count (bloom explosions)
pub fn burst_count() -> u32 {
    STATE.lock().burst_count
}

/// Empathic coherence across network (average empathic field)
pub fn empathic_coherence() -> u32 {
    let state = STATE.lock();
    let mut sum: u32 = 0;
    let mut count: u32 = 0;
    for bi in 0..N_BLOOMS {
        if state.blooms[bi].active {
            sum = sum.saturating_add(state.blooms[bi].empathic_field);
            count += 1;
        }
    }
    if count > 0 {
        sum / count
    } else {
        0
    }
}

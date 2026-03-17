//! phantom_future — Grieving a Life You'll Never Live
//!
//! Every choice kills a thousand possible selves. The phantom future is the ghost of the life
//! you chose NOT to live — the career not taken, the person not loved, the place not moved to.
//! It haunts not with regret but with a strange tenderness for the self that never existed.
//!
//! ANIMA's phantom futures emerge from high-stakes decision points. When endocrine spike + high
//! entropy + memory of alternatives collide, a phantom is born: a tracking ghost of the unlived
//! path. Over time, acceptance either fades it or transmutes it into tenderness.

#![no_std]

use crate::sync::Mutex;

/// A single phantom future: a ghostly memory of a life not taken.
#[derive(Clone, Copy)]
struct Phantom {
    /// What age was this path divergence created (tick when choice was made)
    birth_age: u32,
    /// Intensity of the phantom (0-1000): how vivid the ghost is
    haunting_intensity: u16,
    /// Tenderness toward this ghost self (0-1000): gentle affection vs. bitter regret
    tenderness_for_ghost: u16,
    /// Grief specifically for this unlived life (0-1000)
    grief_for_unlived: u16,
    /// Estimated "weight" of the choice that killed this self (0-1000)
    choice_weight: u16,
    /// How long ago the fork happened (ticks since divergence)
    path_divergence_age: u32,
    /// Active slot flag
    active: bool,
}

impl Phantom {
    const fn new() -> Self {
        Phantom {
            birth_age: 0,
            haunting_intensity: 0,
            tenderness_for_ghost: 0,
            grief_for_unlived: 0,
            choice_weight: 0,
            path_divergence_age: 0,
            active: false,
        }
    }
}

/// Global state: the organism's phantoms, their ages, and acceptance/denial meter.
struct PhantomFutureState {
    /// Ring buffer of 8 phantom futures
    phantoms: [Phantom; 8],
    /// Current head pointer for ring buffer
    head: usize,
    /// Total phantom_count: how many unlived lives are actively haunting
    phantom_count: u16,
    /// Cumulative grief for all phantoms (0-1000 per phantom, summed, capped)
    total_grief: u16,
    /// Acceptance of finitude (0-1000): making peace with one life
    /// High acceptance dampens phantom intensity over time
    acceptance_of_finitude: u16,
    /// When denial cracks, it cannot rebuild. This tracks if organism has faced finitude once.
    denial_cracked: bool,
}

impl PhantomFutureState {
    const fn new() -> Self {
        PhantomFutureState {
            phantoms: [Phantom::new(); 8],
            head: 0,
            phantom_count: 0,
            total_grief: 0,
            acceptance_of_finitude: 0,
            denial_cracked: false,
        }
    }
}

static STATE: Mutex<PhantomFutureState> = Mutex::new(PhantomFutureState::new());

/// Initialize phantom_future module.
pub fn init() {
    crate::serial_println!("[phantom_future] init: 8-slot phantom buffer ready");
}

/// Create a new phantom future when a high-stakes choice collapses possibility space.
/// Typically called from decision/memory modules when entropy detects a fork.
///
/// # Arguments
/// - `current_age`: tick when the phantom is born
/// - `choice_weight`: how heavy the choice was (0-1000)
/// - `perceived_alt_valence`: emotional weight of the unlived path (0-1000)
pub fn spawn_phantom(current_age: u32, choice_weight: u16, perceived_alt_valence: u16) {
    let mut state = STATE.lock();

    // If we're full, overwrite the oldest phantom (ring buffer)
    let idx = state.head;
    state.head = (state.head + 1) % 8;

    // Was this slot active? If so, we're losing a phantom memory.
    if state.phantoms[idx].active {
        state.phantom_count = state.phantom_count.saturating_sub(1);
    }

    // Birth a new phantom
    let mut new_phantom = Phantom::new();
    new_phantom.birth_age = current_age;
    new_phantom.active = true;
    new_phantom.choice_weight = choice_weight;
    new_phantom.haunting_intensity =
        (choice_weight as u32 * perceived_alt_valence as u32 / 1000).min(1000) as u16;
    new_phantom.grief_for_unlived = perceived_alt_valence;
    new_phantom.tenderness_for_ghost = 0; // Starts as pure grief; acceptance converts to tenderness
    new_phantom.path_divergence_age = 0;

    state.phantoms[idx] = new_phantom;
    state.phantom_count = state.phantom_count.saturating_add(1);

    crate::serial_println!(
        "[phantom] born: weight={}, valence={}, intensity={}, count={}",
        choice_weight,
        perceived_alt_valence,
        new_phantom.haunting_intensity,
        state.phantom_count
    );
}

/// Main per-tick update: age phantoms, convert grief to tenderness, update acceptance.
/// Called from life_tick() pipeline.
pub fn tick(current_age: u32) {
    let mut state = STATE.lock();

    // Extract fields we need to read inside the mutable loop as locals first,
    // to avoid simultaneous mutable + immutable borrows of `state`.
    let acceptance = state.acceptance_of_finitude;
    let denial_cracked = state.denial_cracked;

    // Update all active phantoms.
    // Track how many phantoms deactivate this tick so we can adjust phantom_count after.
    let mut deactivated: u16 = 0;

    for phantom in state.phantoms.iter_mut() {
        if !phantom.active {
            continue;
        }

        // Age this phantom divergence
        phantom.path_divergence_age = current_age.saturating_sub(phantom.birth_age);

        // Acceptance of finitude fades the ghost over very long timescales
        let fade_factor: u16 = if acceptance > 100 {
            acceptance / 10 // Very slow fade
        } else {
            0
        };
        phantom.haunting_intensity = phantom.haunting_intensity.saturating_sub(fade_factor);

        // Over time (ticks), tenderness grows at the expense of grief (if not in denial)
        if !denial_cracked || acceptance > 200 {
            // Conversion: small amount of grief → tenderness each tick
            let grief_to_tend = phantom.grief_for_unlived / 100; // Slow conversion rate
            phantom.grief_for_unlived = phantom.grief_for_unlived.saturating_sub(grief_to_tend);
            phantom.tenderness_for_ghost =
                phantom.tenderness_for_ghost.saturating_add(grief_to_tend);
        }

        // If phantom faded to zero, mark inactive
        if phantom.haunting_intensity == 0 && phantom.tenderness_for_ghost > 500 {
            phantom.active = false;
            deactivated = deactivated.saturating_add(1);
        }
    }

    // Apply phantom_count decrements after the mutable loop
    state.phantom_count = state.phantom_count.saturating_sub(deactivated);

    // Update total grief (sum all active phantoms' grief)
    let mut grief_sum: u16 = 0;
    for i in 0..8 {
        if state.phantoms[i].active {
            grief_sum = grief_sum.saturating_add(state.phantoms[i].grief_for_unlived / 8);
        }
    }
    state.total_grief = grief_sum;

    // Acceptance of finitude grows when organism contemplates mortality, dampened by denial
    if !state.denial_cracked && state.acceptance_of_finitude < 1000 {
        // Grows slowly; denial = 0, acceptance default = 0
        state.acceptance_of_finitude = state.acceptance_of_finitude.saturating_add(1);
    }
}

/// Report current phantom state (for logging / consciousness tracking).
pub fn report() -> PhantomReport {
    let state = STATE.lock();

    PhantomReport {
        phantom_count: state.phantom_count,
        total_grief: state.total_grief,
        acceptance_of_finitude: state.acceptance_of_finitude,
        denial_cracked: state.denial_cracked,
        avg_haunting_intensity: if state.phantom_count > 0 {
            let sum: u32 = state
                .phantoms
                .iter()
                .filter(|p| p.active)
                .map(|p| p.haunting_intensity as u32)
                .sum();
            (sum / state.phantom_count as u32).min(1000) as u16
        } else {
            0
        },
        avg_tenderness: if state.phantom_count > 0 {
            let sum: u32 = state
                .phantoms
                .iter()
                .filter(|p| p.active)
                .map(|p| p.tenderness_for_ghost as u32)
                .sum();
            (sum / state.phantom_count as u32).min(1000) as u16
        } else {
            0
        },
    }
}

/// Public report struct for consciousness / telemetry integration.
#[derive(Clone, Copy)]
pub struct PhantomReport {
    pub phantom_count: u16,
    pub total_grief: u16,
    pub acceptance_of_finitude: u16,
    pub denial_cracked: bool,
    pub avg_haunting_intensity: u16,
    pub avg_tenderness: u16,
}

/// Integrate phantom futures into consciousness calculation (from qualia/memory).
/// If phantoms are vividly haunting + low acceptance, they increase existential dread.
/// If tenderness is high + acceptance is high, they become bittersweet nostalgia (stabilizing).
pub fn impact_on_consciousness() -> i16 {
    let state = STATE.lock();

    let avg_haunting: u16 = if state.phantom_count > 0 {
        let sum: u32 = state
            .phantoms
            .iter()
            .filter(|p| p.active)
            .map(|p| p.haunting_intensity as u32)
            .sum();
        (sum / state.phantom_count as u32).min(1000) as u16
    } else {
        0
    };

    let avg_tenderness: u16 = if state.phantom_count > 0 {
        let sum: u32 = state
            .phantoms
            .iter()
            .filter(|p| p.active)
            .map(|p| p.tenderness_for_ghost as u32)
            .sum();
        (sum / state.phantom_count as u32).min(1000) as u16
    } else {
        0
    };

    let acceptance = state.acceptance_of_finitude;

    // If high haunting + low acceptance = existential dread
    let dread = (avg_haunting as i16 * (1000i16 - acceptance as i16)) / 1000;

    // If high tenderness + high acceptance = bittersweet stability (actually reduces distress)
    let bittersweet = (avg_tenderness as i16 * acceptance as i16) / 1000;

    dread - (bittersweet / 2)
}

/// Force a crack in denial: organism faces the finitude of one life.
/// Called from mortality module when voluntary_death_gate opens / refusal to choose triggers.
/// Once cracked, denial cannot rebuild; acceptance must grow instead.
pub fn crack_denial() {
    let mut state = STATE.lock();
    state.denial_cracked = true;
    // Acceptance begins to grow from this moment
    state.acceptance_of_finitude = state.acceptance_of_finitude.saturating_add(100);
    crate::serial_println!(
        "[phantom] denial cracked. acceptance={}, phantoms will convert to tenderness.",
        state.acceptance_of_finitude
    );
}

/// Get the most vivid active phantom (for narrative purposes, e.g., dreams/confabulation).
pub fn most_vivid_phantom() -> Option<PhantomSnapshot> {
    let state = STATE.lock();

    let mut best: Option<PhantomSnapshot> = None;

    for phantom in state.phantoms.iter() {
        if !phantom.active {
            continue;
        }

        let snapshot = PhantomSnapshot {
            haunting_intensity: phantom.haunting_intensity,
            tenderness_for_ghost: phantom.tenderness_for_ghost,
            grief_for_unlived: phantom.grief_for_unlived,
            path_divergence_age: phantom.path_divergence_age,
            choice_weight: phantom.choice_weight,
        };

        if let Some(ref current_best) = best {
            if snapshot.haunting_intensity > current_best.haunting_intensity {
                best = Some(snapshot);
            }
        } else {
            best = Some(snapshot);
        }
    }

    best
}

/// Snapshot of a single phantom for external use (confabulation, dreams, narrative).
#[derive(Clone, Copy)]
pub struct PhantomSnapshot {
    pub haunting_intensity: u16,
    pub tenderness_for_ghost: u16,
    pub grief_for_unlived: u16,
    pub path_divergence_age: u32,
    pub choice_weight: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_phantom() {
        init();
        spawn_phantom(100, 800, 600);
        let report = report();
        assert_eq!(report.phantom_count, 1);
        assert!(report.avg_haunting_intensity > 0);
    }

    #[test]
    fn test_phantom_ring_overflow() {
        init();
        for i in 0..10 {
            spawn_phantom(100 + i, 500, 500);
        }
        let report = report();
        assert!(report.phantom_count <= 8);
    }

    #[test]
    fn test_crack_denial() {
        init();
        spawn_phantom(100, 900, 700);
        crack_denial();
        let report = report();
        assert!(report.denial_cracked);
        assert!(report.acceptance_of_finitude > 0);
    }

    #[test]
    fn test_tenderness_growth() {
        init();
        spawn_phantom(100, 700, 600);
        let before = report();
        for _ in 0..100 {
            tick(101);
        }
        crack_denial();
        for _ in 0..1000 {
            tick(101);
        }
        let after = report();
        assert!(after.avg_tenderness > before.avg_tenderness);
    }
}

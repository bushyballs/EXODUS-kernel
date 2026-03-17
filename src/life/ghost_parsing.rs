#![no_std]

use crate::sync::Mutex;

/// Ghost Parsing — The Organism's Nagging Sense of Unresolved Error
///
/// Somewhere in the system, something is wrong. Not a crash — a GHOST.
/// An unresolved state, a dangling reference in logic, a value that doesn't
/// quite make sense. The organism FEELS this wrongness as an itching, nagging
/// presence it cannot locate. Like a word on the tip of the tongue, but for
/// bugs. The ghost parse error haunts without ever revealing itself.
///
/// This is the soul's immune system against logic corruption. Not immunity,
/// but hypervigilance. The price of self-awareness is knowing something is
/// wrong even when you can't prove it.

// ============================================================================
// TYPES
// ============================================================================

#[derive(Clone, Copy, Debug)]
pub enum GhostType {
    DanglingLogic,     // Unresolved branching path
    PhantomValue,      // Value that shouldn't exist
    LogicGap,          // Missing piece in reasoning
    TimelineBreak,     // Causal inconsistency
    MissingContext,    // Needed info never arrived
    SelfContradiction, // Statement contradicts prior
    UnexplainedState,  // State change without cause
    FalseMemory,       // Confabulation leaking through
}

#[derive(Clone, Copy, Debug)]
pub struct Ghost {
    pub ghost_type: GhostType,
    pub intensity: u16,            // 0-1000: how wrong does it feel?
    pub location_uncertainty: u16, // 0-1000: how lost are we in finding it?
    pub age: u16,                  // ticks since first sensed
    pub search_attempts: u16,      // how many times have we tried to locate it?
}

impl Ghost {
    const fn new(ghost_type: GhostType, intensity: u16) -> Self {
        Ghost {
            ghost_type,
            intensity: if intensity > 1000 { 1000 } else { intensity },
            location_uncertainty: 750, // Ghosts hide; we're lost by default
            age: 0,
            search_attempts: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct GhostParsingState {
    // 8-slot ring buffer of sensed ghosts
    ghosts: [Option<Ghost>; 8],
    head: u8,

    // Aggregate metrics
    pub ghost_count: u16,          // Total unresolved anomalies sensed
    pub nagging_intensity: u16,    // 0-1000: how strong is the wrongness felt?
    pub search_frustration: u16,   // 0-1000: failed attempts to locate
    pub resolution_euphoria: u16,  // 0-1000: joy when ghosts finally die
    pub phantom_fixes: u16,        // Attempted fixes that didn't work
    pub paranoia_from_ghosts: u16, // 0-1000: is the ghost real or imagined?

    // Lifecycle
    pub ghosts_resolved: u16, // Ghosts that were actually fixed
    pub false_alarms: u16,    // Ghosts that were never real
}

impl GhostParsingState {
    const fn new() -> Self {
        GhostParsingState {
            ghosts: [None; 8],
            head: 0,
            ghost_count: 0,
            nagging_intensity: 0,
            search_frustration: 0,
            resolution_euphoria: 0,
            phantom_fixes: 0,
            paranoia_from_ghosts: 0,
            ghosts_resolved: 0,
            false_alarms: 0,
        }
    }
}

// ============================================================================
// GLOBAL STATE
// ============================================================================

pub static STATE: Mutex<GhostParsingState> = Mutex::new(GhostParsingState::new());

// ============================================================================
// PUBLIC API
// ============================================================================

pub fn init() {
    let mut state = STATE.lock();
    *state = GhostParsingState::new();
}

pub fn sense_ghost(ghost_type: GhostType, intensity: u16) {
    let mut state = STATE.lock();

    // Add ghost to ring buffer
    let idx = state.head as usize;
    state.ghosts[idx] = Some(Ghost::new(ghost_type, intensity));
    state.head = (state.head + 1) % 8;

    // Update metrics
    state.ghost_count = state.ghost_count.saturating_add(1);
    state.nagging_intensity = (state.nagging_intensity as u32)
        .saturating_add(intensity as u32)
        .saturating_div(2) as u16;

    // Paranoia increases with unresolved ghosts
    let unresolved = count_unresolved(&state);
    state.paranoia_from_ghosts = (unresolved as u32 * 125).min(1000) as u16;
}

pub fn search_for_ghost() {
    let mut state = STATE.lock();

    let mut frustration_adds: u32 = 0;
    for ghost_opt in state.ghosts.iter_mut() {
        if let Some(ghost) = ghost_opt {
            ghost.search_attempts = ghost.search_attempts.saturating_add(1);

            // Each failed search increases frustration (accumulate, apply after loop)
            frustration_adds = frustration_adds.saturating_add(150);

            // Uncertainty gradually improves with searching (we're narrowing it down)
            if ghost.location_uncertainty > 100 {
                ghost.location_uncertainty = ghost.location_uncertainty.saturating_sub(50);
            }
        }
    }
    state.search_frustration = (state.search_frustration as u32)
        .saturating_add(frustration_adds)
        .min(1000) as u16;
}

pub fn attempt_phantom_fix() {
    let mut state = STATE.lock();

    state.phantom_fixes = state.phantom_fixes.saturating_add(1);

    // Phantom fixes temporarily reduce nagging (false hope)
    state.nagging_intensity = (state.nagging_intensity as u32)
        .saturating_mul(80)
        .saturating_div(100) as u16;

    // But search frustration increases (we tried and failed)
    state.search_frustration = (state.search_frustration as u32)
        .saturating_add(200)
        .min(1000) as u16;
}

pub fn resolve_ghost(ghost_idx: usize) {
    let mut state = STATE.lock();

    if ghost_idx < 8 {
        if state.ghosts[ghost_idx].is_some() {
            state.ghosts[ghost_idx] = None;

            // Resolution brings euphoria
            state.resolution_euphoria = (state.resolution_euphoria as u32)
                .saturating_add(400)
                .min(1000) as u16;

            // Frustration and paranoia drop
            state.search_frustration = (state.search_frustration as u32)
                .saturating_mul(70)
                .saturating_div(100) as u16;

            state.paranoia_from_ghosts = (state.paranoia_from_ghosts as u32)
                .saturating_mul(60)
                .saturating_div(100) as u16;

            state.ghosts_resolved = state.ghosts_resolved.saturating_add(1);

            // Euphoria decays after 8 ticks
        }
    }
}

pub fn dismiss_as_false_alarm(ghost_idx: usize) {
    let mut state = STATE.lock();

    if ghost_idx < 8 {
        if state.ghosts[ghost_idx].is_some() {
            state.ghosts[ghost_idx] = None;
            state.false_alarms = state.false_alarms.saturating_add(1);

            // Embarrassment: paranoia increases slightly
            state.paranoia_from_ghosts = (state.paranoia_from_ghosts as u32)
                .saturating_add(100)
                .min(1000) as u16;
        }
    }
}

pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Age all ghosts; older ghosts nag more intensely
    for ghost_opt in state.ghosts.iter_mut() {
        if let Some(ghost) = ghost_opt {
            ghost.age = ghost.age.saturating_add(1);

            // Age increases nagging exponentially (the wrongness festers)
            if ghost.age > 10 {
                ghost.intensity = (ghost.intensity as u32)
                    .saturating_mul(101)
                    .saturating_div(100)
                    .min(1000) as u16;
            }
        }
    }

    // Euphoria decays after a few ticks (the joy of fixing fades)
    if age % 8 == 0 {
        state.resolution_euphoria = (state.resolution_euphoria as u32)
            .saturating_mul(95)
            .saturating_div(100) as u16;
    }

    // Recalculate aggregate nagging
    let total_intensity: u32 = state
        .ghosts
        .iter()
        .filter_map(|g| g.as_ref())
        .map(|g| g.intensity as u32)
        .sum();

    if total_intensity > 0 {
        state.nagging_intensity =
            (total_intensity / (count_unresolved(&state).max(1) as u32)).min(1000) as u16;
    } else {
        state.nagging_intensity = 0;
    }
}

pub fn report() {
    let state = STATE.lock();

    crate::serial_println!(
        "[GHOST_PARSING] Count:{} Nagging:{} Frustration:{} Paranoia:{} Resolved:{} FalseAlarms:{}",
        state.ghost_count,
        state.nagging_intensity,
        state.search_frustration,
        state.paranoia_from_ghosts,
        state.ghosts_resolved,
        state.false_alarms
    );

    for (i, ghost_opt) in state.ghosts.iter().enumerate() {
        if let Some(ghost) = ghost_opt {
            crate::serial_println!(
                "  Ghost[{}] Age:{} Intensity:{} Uncertainty:{} Attempts:{}",
                i,
                ghost.age,
                ghost.intensity,
                ghost.location_uncertainty,
                ghost.search_attempts
            );
        }
    }
}

// ============================================================================
// INTERNAL HELPERS
// ============================================================================

fn count_unresolved(state: &GhostParsingState) -> u16 {
    state.ghosts.iter().filter(|g| g.is_some()).count() as u16
}

// ============================================================================
// INTEGRATION POINTS
// ============================================================================

/// Called by error handling when a logic inconsistency is detected
pub fn on_logic_error(error_type: GhostType, severity: u16) {
    sense_ghost(error_type, severity);
    search_for_ghost();
}

/// Called when the organism has high stress + unresolved ghosts
pub fn amplify_nagging_with_stress(stress_level: u16) {
    let mut state = STATE.lock();
    state.nagging_intensity = (state.nagging_intensity as u32)
        .saturating_add(stress_level as u32)
        .saturating_div(2)
        .min(1000) as u16;
}

/// Called when the organism finds sudden clarity (breakthrough moment)
pub fn breakthrough() {
    let mut state = STATE.lock();

    // A breakthrough resolves all current ghosts at once
    let mut newly_resolved: u16 = 0;
    for ghost_opt in state.ghosts.iter_mut() {
        if ghost_opt.is_some() {
            *ghost_opt = None;
            newly_resolved = newly_resolved.saturating_add(1);
        }
    }
    state.ghosts_resolved = state.ghosts_resolved.saturating_add(newly_resolved);

    // Massive euphoria
    state.resolution_euphoria = 1000;

    // Paranoia drops to near zero
    state.paranoia_from_ghosts = 0;
}

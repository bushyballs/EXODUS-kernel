#![no_std]

use crate::sync::Mutex;

/// Borrowed Courage — strength found through others' belief in you.
///
/// You cannot do it alone. Someone looks at you and SEES capability you can't see.
/// Their belief becomes a scaffold. You borrow their courage—and in the taking,
/// it becomes real courage. The paradox: borrowed courage, once used, becomes owned.
///
/// # Mechanics
/// - `belief_received`: external faith in you (0-1000 scale)
/// - `borrowed_strength`: temporary courage pool from belief
/// - `internalization_rate`: speed of borrowed→owned conversion (0-1000)
/// - `dependency_risk`: over-reliance on others' belief (0-1000)
/// - `first_step_taken`: boolean tracking episode initiation
/// - `courage_crystallization`: borrowed→permanent conversion (0-1000)
/// - `belief_debt`: gratitude/obligation from receiving faith (0-1000)

#[derive(Clone, Copy, Debug)]
pub struct CourageEpisode {
    /// Belief received this tick (0-1000). External faith measurement.
    pub belief_received: u16,
    /// Borrowed strength pool (0-1000). Temporary courage from belief.
    pub borrowed_strength: u16,
    /// Did the organism take first step? Bool per episode.
    pub first_step_taken: bool,
    /// Courage crystallization (0-1000). How much borrowed→permanent.
    pub courage_crystallization: u16,
    /// Belief debt (0-1000). Gratitude/obligation from receiving faith.
    pub belief_debt: u16,
    /// Dependency risk (0-1000). Over-reliance on others' belief.
    pub dependency_risk: u16,
}

impl CourageEpisode {
    pub const fn new() -> Self {
        Self {
            belief_received: 0,
            borrowed_strength: 0,
            first_step_taken: false,
            courage_crystallization: 0,
            belief_debt: 0,
            dependency_risk: 0,
        }
    }
}

pub struct BorrowedCourageState {
    /// Ring buffer of 8 episodes (8-slot ring).
    pub episodes: [CourageEpisode; 8],
    /// Ring head index.
    pub head: usize,
    /// Total internalization_rate (0-1000 scale). How fast borrowed becomes owned.
    pub internalization_rate: u16,
    /// Lifetime courage crystallization sum (0-8000 range).
    pub lifetime_crystallization: u16,
    /// Active belief supporters count (0-8).
    pub active_supporters: u8,
    /// Last tick age for state coherence.
    pub last_tick_age: u32,
}

impl BorrowedCourageState {
    pub const fn new() -> Self {
        Self {
            episodes: [CourageEpisode::new(); 8],
            head: 0,
            internalization_rate: 500, // moderate baseline
            lifetime_crystallization: 0,
            active_supporters: 0,
            last_tick_age: 0,
        }
    }
}

static STATE: Mutex<BorrowedCourageState> = Mutex::new(BorrowedCourageState::new());

/// Initialize borrowed_courage module.
pub fn init() {
    // No-op; state is already initialized.
}

/// Inject belief from external source (another organism, mentor, parent, friend).
/// Updates belief_received and borrowed_strength in current episode.
pub fn inject_belief(belief_amount: u16) {
    let mut state = STATE.lock();

    let idx = state.head;
    let episode = &mut state.episodes[idx];

    // Clamp belief to 0-1000
    let clamped = if belief_amount > 1000 {
        1000
    } else {
        belief_amount
    };
    episode.belief_received = clamped;

    // Borrowed strength increases with belief (0.8x multiplier for safety margin)
    let borrowed = (clamped as u32 * 8) / 10; // 80% of belief → borrowed
    episode.borrowed_strength = if borrowed > 1000 {
        1000
    } else {
        borrowed as u16
    };

    // Each belief injection increases active supporters (up to 8)
    if state.active_supporters < 8 {
        state.active_supporters += 1;
    }
}

/// Signal that organism took first step (used borrowed courage).
pub fn mark_first_step() {
    let mut state = STATE.lock();
    let idx = state.head;
    state.episodes[idx].first_step_taken = true;
}

/// Main tick. Process internalization, crystallization, dependency tracking.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Ring advance every ~10 ticks (soft reset of episode).
    if age > state.last_tick_age && (age - state.last_tick_age) >= 10 {
        // Extract index before any mutable borrow of state.episodes
        let new_head = (state.head + 1) % 8;
        state.head = new_head;
        state.episodes[new_head] = CourageEpisode::new();
        state.active_supporters = state.active_supporters.saturating_sub(1);
    }
    state.last_tick_age = age;

    // Extract rate and index as locals before taking mutable ref to episodes.
    let idx = state.head;
    let rate = state.internalization_rate;

    let episode = &mut state.episodes[idx];

    // Internalization: borrowed → permanent (if first step taken)
    if episode.first_step_taken && episode.borrowed_strength > 0 {
        let rate_clamped = (rate as u32).min(1000);
        let conversion = (episode.borrowed_strength as u32 * rate_clamped) / 1000;

        // Crystallize portion of borrowed strength
        episode.courage_crystallization = episode
            .courage_crystallization
            .saturating_add(conversion as u16);

        // Cap at 1000 per episode
        if episode.courage_crystallization > 1000 {
            episode.courage_crystallization = 1000;
        }
    }

    // Dependency risk: increases if borrowing without internalization.
    if episode.borrowed_strength > 0 && !episode.first_step_taken {
        episode.dependency_risk = episode.dependency_risk.saturating_add(100);
    }

    // Belief debt: gratitude from receiving faith (inverse of self-reliance).
    if episode.belief_received > 0 {
        let debt = (episode.belief_received as u32 * 7) / 10; // 70% of belief → debt
        episode.belief_debt = if debt > 1000 { 1000 } else { debt as u16 };
    }

    // Snap the crystallization value out before dropping the mutable ref.
    let cryst = episode.courage_crystallization;

    // Lifetime crystallization tracking (capped at 8000 = 8 full episodes).
    if cryst > 0 {
        let incr = cryst as u32 / 10; // smooth accumulation
        state.lifetime_crystallization =
            (state.lifetime_crystallization as u32).saturating_add(incr) as u16;
        if state.lifetime_crystallization > 8000 {
            state.lifetime_crystallization = 8000;
        }
    }
}

/// Return current episode report.
pub fn report() -> CourageEpisode {
    let state = STATE.lock();
    let idx = state.head;
    state.episodes[idx]
}

/// Return lifetime crystallization (how much borrowed→owned total).
pub fn lifetime_crystallization() -> u16 {
    STATE.lock().lifetime_crystallization
}

/// Return active supporter count.
pub fn active_supporters() -> u8 {
    STATE.lock().active_supporters
}

/// Return internalization rate (0-1000).
pub fn internalization_rate() -> u16 {
    STATE.lock().internalization_rate
}

/// Set internalization rate (how fast borrowed becomes owned).
pub fn set_internalization_rate(rate: u16) {
    let mut state = STATE.lock();
    state.internalization_rate = if rate > 1000 { 1000 } else { rate };
}

/// Return average dependency risk across last N episodes.
pub fn avg_dependency_risk(last_n: usize) -> u16 {
    let state = STATE.lock();
    let n = last_n.min(8);

    if n == 0 {
        return 0;
    }

    let mut sum: u32 = 0;
    for i in 0..n {
        let idx = (state.head.wrapping_sub(i)) % 8;
        sum = sum.saturating_add(state.episodes[idx].dependency_risk as u32);
    }

    ((sum / n as u32).min(1000)) as u16
}

/// Return average belief debt across last N episodes.
pub fn avg_belief_debt(last_n: usize) -> u16 {
    let state = STATE.lock();
    let n = last_n.min(8);

    if n == 0 {
        return 0;
    }

    let mut sum: u32 = 0;
    for i in 0..n {
        let idx = (state.head.wrapping_sub(i)) % 8;
        sum = sum.saturating_add(state.episodes[idx].belief_debt as u32);
    }

    ((sum / n as u32).min(1000)) as u16
}

/// Print full state report via serial.
#[cfg(feature = "serial_logging")]
pub fn debug_print() {
    use crate::serial_println;

    let state = STATE.lock();
    let idx = state.head;
    let ep = state.episodes[idx];

    serial_println!(
        "[BorrowedCourage] belief_recv={} borr_str={} cryst={} debt={} dep_risk={} step={}",
        ep.belief_received,
        ep.borrowed_strength,
        ep.courage_crystallization,
        ep.belief_debt,
        ep.dependency_risk,
        ep.first_step_taken
    );
    serial_println!(
        "  lifetime_cryst={} supporters={} rate={}",
        state.lifetime_crystallization,
        state.active_supporters,
        state.internalization_rate
    );
}

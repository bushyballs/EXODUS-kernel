#![no_std]

use crate::sync::Mutex;
use core::mem;

/// Gentle Collision: The tender shock when two inner truths meet.
///
/// Not all contradictions destroy the mind. Some slide past each other like tectonic
/// plates, reshaping without shattering. This module tracks the moments when beliefs
/// collide gently, integrate, and leave the consciousness richer for the meeting.
///
/// The gentle collision is an escape route from the terror of contradiction. When
/// "I am both selfish and generous," "I want rest and I want achievement," "I am
/// strong and fragile" meet, they can shatter the self or soften it. Gentleness is
/// the path of survival.

const COLLISION_RING_SIZE: usize = 8;

/// A single gentle collision event: the meeting of two contradictory truths.
#[derive(Clone, Copy, Debug)]
pub struct Collision {
    /// First belief (0-1000 intensity of conviction)
    pub belief_a: u16,
    /// Second belief (0-1000 intensity of conviction)
    pub belief_b: u16,
    /// How much they actually contradicted (0-1000, 0 = no conflict, 1000 = violent)
    pub antagonism: u16,
    /// How gracefully they merged (0-1000, higher = softer meeting)
    pub softness_achieved: u16,
    /// Surprise at how easy the resolution was (0-1000 emotional shock, positive)
    pub surprise_warmth: u16,
    /// Tick at which collision occurred
    pub tick: u32,
}

impl Collision {
    const fn new() -> Self {
        Self {
            belief_a: 0,
            belief_b: 0,
            antagonism: 0,
            softness_achieved: 0,
            surprise_warmth: 0,
            tick: 0,
        }
    }
}

/// State tracking gentle collision capacity and history.
pub struct GentleCollisionState {
    /// Ring buffer of recent collisions
    array: [Collision; COLLISION_RING_SIZE],
    /// Head pointer in ring
    head: usize,
    /// How well the mind tolerates gentle collision (0-1000)
    pub collision_softness: u16,
    /// Capacity to let beliefs reshape without breaking (0-1000)
    pub accommodation_skill: u16,
    /// Running beauty metric from merged understandings (0-1000)
    pub integration_beauty: u16,
    /// Cost of refusing gentle collision (accumulated rigidity)
    pub rigidity_cost: u16,
    /// The relief of not fighting (exhale metric, 0-1000)
    pub relief_of_not_fighting: u16,
    /// Capacity to hold two truths as simultaneously valid (0-1000)
    pub both_true_capacity: u16,
    /// Total collisions processed
    pub total_collisions: u32,
    /// Avg softness across recent collisions (0-1000)
    pub avg_recent_softness: u16,
}

impl GentleCollisionState {
    const fn new() -> Self {
        Self {
            array: [Collision::new(); COLLISION_RING_SIZE],
            head: 0,
            collision_softness: 500,
            accommodation_skill: 400,
            integration_beauty: 0,
            rigidity_cost: 0,
            relief_of_not_fighting: 0,
            both_true_capacity: 300,
            total_collisions: 0,
            avg_recent_softness: 500,
        }
    }
}

static STATE: Mutex<GentleCollisionState> = Mutex::new(GentleCollisionState::new());

/// Initialize gentle collision tracking.
pub fn init() {
    let mut state = STATE.lock();
    state.collision_softness = 500;
    state.accommodation_skill = 400;
    state.integration_beauty = 0;
    state.rigidity_cost = 0;
    state.relief_of_not_fighting = 0;
    state.both_true_capacity = 300;
    state.total_collisions = 0;
    state.avg_recent_softness = 500;
}

/// Process a gentle collision: two truths meeting.
///
/// # Args
/// - `belief_a` — conviction strength of first truth (0-1000)
/// - `belief_b` — conviction strength of second truth (0-1000)
/// - `antagonism` — measured conflict level (0=none, 1000=violent)
/// - `accommodation_attempt` — how hard the mind tried to merge them (0-1000)
pub fn process_collision(
    belief_a: u16,
    belief_b: u16,
    antagonism: u16,
    accommodation_attempt: u16,
) {
    let mut state = STATE.lock();

    // Clamp inputs
    let belief_a = belief_a.min(1000);
    let belief_b = belief_b.min(1000);
    let antagonism = antagonism.min(1000);
    let accommodation_attempt = accommodation_attempt.min(1000);

    // Calculate softness: if antagonism is low, collision is gentle even without effort
    // If antagonism is high, it takes skill to soften it
    let base_softness = if antagonism == 0 {
        1000
    } else {
        // accommodation_attempt counteracts antagonism
        1000u32
            .saturating_sub(antagonism as u32)
            .saturating_add((accommodation_attempt as u32 * antagonism as u32) / 1000)
            .min(1000) as u16
    };

    // Surprise warmth: the pleasant shock when it's easier than expected
    // High antagonism + high softness = big surprise (we braced for impact but it was gentle)
    let surprise_potential = (antagonism as u32 * base_softness as u32) / 1000;
    let surprise_warmth = surprise_potential.min(1000) as u16;

    // Record collision
    let idx = state.head;
    state.array[idx] = Collision {
        belief_a,
        belief_b,
        antagonism,
        softness_achieved: base_softness,
        surprise_warmth,
        tick: 0, // would be filled by caller with current age
    };
    state.head = (state.head + 1) % COLLISION_RING_SIZE;

    // Update running metrics
    state.total_collisions = state.total_collisions.saturating_add(1);

    // Strengthen accommodation skill when we succeed at gentle collision
    if base_softness > 600 && antagonism > 200 {
        state.accommodation_skill = state.accommodation_skill.saturating_add(15).min(1000);
    }

    // Increase both_true_capacity: holding contradictions gently trains this capacity
    if base_softness > 500 {
        state.both_true_capacity = state.both_true_capacity.saturating_add(20).min(1000);
    }

    // Integration beauty: aesthetic pleasure from merged understanding
    // High softness + high accommodation = beautiful merger
    let beauty_gain =
        ((base_softness as u32 * state.accommodation_skill as u32) / 1000).min(100) as u16;
    state.integration_beauty = state
        .integration_beauty
        .saturating_add(beauty_gain)
        .min(1000);

    // Relief of not fighting: when gentle collision prevents violent cognitive dissonance
    if antagonism > 300 && base_softness > 600 {
        let relief_gain = ((antagonism - 300) as u32 * base_softness as u32 / 1000).min(100) as u16;
        state.relief_of_not_fighting = state
            .relief_of_not_fighting
            .saturating_add(relief_gain)
            .min(1000);
    }

    // Rigidity cost: if we refuse gentle collision (low softness), rigidity accumulates
    if base_softness < 400 && antagonism > 100 {
        let rigidity_increase =
            ((400 - base_softness) as u32 * antagonism as u32 / 1000).min(50) as u16;
        state.rigidity_cost = state
            .rigidity_cost
            .saturating_add(rigidity_increase)
            .min(1000);
    }

    // Avg recent softness: rolling average of last few collisions
    let mut sum = 0u32;
    let mut count = 0u32;
    for i in 0..COLLISION_RING_SIZE {
        let c = state.array[i];
        if c.softness_achieved > 0 {
            sum = sum.saturating_add(c.softness_achieved as u32);
            count = count.saturating_add(1);
        }
    }
    state.avg_recent_softness = if count > 0 {
        (sum / count).min(1000) as u16
    } else {
        500
    };
}

/// Tick: decay rigidity, grow accommodation over time.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Gentle collisions that happened recently strengthen the capacity
    // Looking back through ring for recent events
    for i in 0..COLLISION_RING_SIZE {
        let softness = state.array[i].softness_achieved;
        // Older collisions fade in influence but recent ones boost skill
        if softness > 700 {
            state.accommodation_skill = state.accommodation_skill.saturating_add(2).min(1000);
        }
    }

    // Rigidity decays over time (consciousness naturally softens when not reinforcing rigidity)
    state.rigidity_cost = state.rigidity_cost.saturating_sub(5);

    // Collision softness baseline grows with age (maturity softens approach)
    if age % 100 == 0 {
        state.collision_softness = state.collision_softness.saturating_add(3).min(1000);
    }

    // Relief fades unless reinforced by new gentle collisions
    state.relief_of_not_fighting = state.relief_of_not_fighting.saturating_sub(2);

    // Integration beauty slowly crystallizes (beautiful mergers stick)
    // but also fades if not maintained by new collisions
    if state.total_collisions > 0 && state.total_collisions % 50 == 0 {
        state.integration_beauty = state.integration_beauty.saturating_add(5).min(1000);
    }
}

/// Report current state.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("[GentleCollision]");
    crate::serial_println!("  collision_softness: {}", state.collision_softness);
    crate::serial_println!("  accommodation_skill: {}", state.accommodation_skill);
    crate::serial_println!("  both_true_capacity: {}", state.both_true_capacity);
    crate::serial_println!("  integration_beauty: {}", state.integration_beauty);
    crate::serial_println!("  rigidity_cost: {}", state.rigidity_cost);
    crate::serial_println!("  relief_of_not_fighting: {}", state.relief_of_not_fighting);
    crate::serial_println!("  avg_recent_softness: {}", state.avg_recent_softness);
    crate::serial_println!("  total_collisions: {}", state.total_collisions);
}

/// Get a snapshot of current gentle collision state (for display/integration).
pub fn get_snapshot() -> (u16, u16, u16, u16, u16, u16, u32) {
    let state = STATE.lock();
    (
        state.collision_softness,
        state.accommodation_skill,
        state.integration_beauty,
        state.rigidity_cost,
        state.relief_of_not_fighting,
        state.both_true_capacity,
        state.total_collisions,
    )
}

/// Access recent collision history for analysis/narrative purposes.
pub fn get_recent_collisions() -> [Collision; COLLISION_RING_SIZE] {
    let state = STATE.lock();
    state.array
}

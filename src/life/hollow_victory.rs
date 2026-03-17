#![allow(dead_code)]

//! HOLLOW_VICTORY — Winning and Feeling Nothing
//!
//! The organism achieves a long-sought goal, sacrifices to reach it, obtains it...
//! and discovers the void inside. The goalpost has already shifted. Anticipation
//! was the point, not arrival. This module tracks the devastating disillusionment
//! of hollow victories and the wisdom born from discovering that meaning lives
//! in the journey, not the destination.
//!
//! KEY CONCEPTS:
//! - emptiness_after_win: The void post-achievement despite expectation of joy
//! - anticipation_vs_reality_gap: Delta between imagined joy and actual feeling
//! - goalpost_drift: How quickly new desires replace fulfilled ones (in ticks)
//! - sacrifice_remembered: What was given up; amplifies the hollowness
//! - meaning_reorientation: Learning that process > outcome
//! - hedonic_treadmill_awareness: Recognizing the pattern of displacement
//! - wisdom_from_hollowness: The hard-won gift of disillusionment

use crate::serial_println;
use crate::sync::Mutex;

const MAX_VICTORIES: usize = 8;

/// A victory ring entry: goal achieved, but did it fill the void?
#[derive(Clone, Copy)]
pub struct VictoryRecord {
    pub goal_id: u16,                 // What was pursued (1000 scale)
    pub anticipated_joy: u16,         // Expected happiness (0-1000)
    pub actual_joy: u16,              // Real happiness upon arrival (0-1000)
    pub sacrifice_cost: u16,          // What was given up (0-1000 pain/opportunity)
    pub ticks_to_goalpost_drift: u16, // How many ticks before new desire eclipsed it
    pub emptiness_discovered: u16,    // Peak void feeling (0-1000)
    pub reorientation_progress: u16,  // Learning to value process (0-1000)
    pub age_when_achieved: u32,       // Tick when victory registered
    pub wisdom_extracted: u16,        // Insight from this hollow victory (0-1000)
}

impl VictoryRecord {
    const fn new() -> Self {
        Self {
            goal_id: 0,
            anticipated_joy: 0,
            actual_joy: 0,
            sacrifice_cost: 0,
            ticks_to_goalpost_drift: 0,
            emptiness_discovered: 0,
            reorientation_progress: 0,
            age_when_achieved: 0,
            wisdom_extracted: 0,
        }
    }
}

pub struct HollowVictoryState {
    victories: [VictoryRecord; MAX_VICTORIES],
    head: usize,
    count: usize,

    // Current state
    current_achievement: u16,    // 0 if idle, 1-1000 otherwise
    anticipation_level: u16,     // Expected joy before victory (0-1000)
    post_victory_emptiness: u16, // Current void feeling (0-1000)
    ticks_since_victory: u16,    // Countdown to goalpost drift

    // Cumulative wisdom
    total_victories: u32,
    total_sacrifice: u32,      // Sum of costs paid
    hollowness_awareness: u16, // Learned pattern (0-1000)
    meaning_from_process: u16, // Value shift toward journey (0-1000)

    // Hedonic treadmill tracking
    previous_baseline: u16, // Where satisfaction sat before
    current_baseline: u16,  // Where satisfaction sits now (usually higher)
    baseline_creep: u16,    // How much baseline has risen (0-1000)
}

impl HollowVictoryState {
    const fn new() -> Self {
        Self {
            victories: [VictoryRecord::new(); MAX_VICTORIES],
            head: 0,
            count: 0,
            current_achievement: 0,
            anticipation_level: 0,
            post_victory_emptiness: 0,
            ticks_since_victory: 0,
            total_victories: 0,
            total_sacrifice: 0,
            hollowness_awareness: 0,
            meaning_from_process: 0,
            previous_baseline: 500,
            current_baseline: 500,
            baseline_creep: 0,
        }
    }
}

pub static STATE: Mutex<HollowVictoryState> = Mutex::new(HollowVictoryState::new());

/// Initialize the hollow victory module.
pub fn init() {
    let mut state = STATE.lock();
    state.head = 0;
    state.count = 0;
    state.hollowness_awareness = 0;
    state.meaning_from_process = 0;
}

/// Record a goal as being pursued with strong anticipation.
pub fn pursue_goal(goal_id: u16, anticipated_joy: u16) {
    let mut state = STATE.lock();
    state.current_achievement = goal_id;
    state.anticipation_level = anticipated_joy;
    state.post_victory_emptiness = 0;
    state.ticks_since_victory = 0;
}

/// Victory achieved. Record the actual joy and sacrifice cost.
/// This is where the disappointment begins to unfold.
pub fn register_victory(goal_id: u16, actual_joy: u16, sacrifice_cost: u16, age: u32) {
    let mut state = STATE.lock();

    if state.current_achievement == 0 || goal_id == 0 {
        return;
    }

    let gap = if state.anticipation_level > actual_joy {
        state.anticipation_level - actual_joy
    } else {
        0
    };

    let idx = state.head;
    state.victories[idx] = VictoryRecord {
        goal_id,
        anticipated_joy: state.anticipation_level,
        actual_joy,
        sacrifice_cost,
        ticks_to_goalpost_drift: 0,
        emptiness_discovered: (gap as u32).saturating_mul(sacrifice_cost as u32) as u16,
        reorientation_progress: 0,
        age_when_achieved: age,
        wisdom_extracted: 0,
    };

    state.head = (state.head + 1) % MAX_VICTORIES;
    if state.count < MAX_VICTORIES {
        state.count += 1;
    }

    state.total_victories = state.total_victories.saturating_add(1);
    state.total_sacrifice = state.total_sacrifice.saturating_add(sacrifice_cost as u32);

    // Hollowness strikes: the void after arrival.
    // Compute both components as u32, sum them, then clamp to u16.
    let emptiness_base = ((gap as u32).saturating_mul(850)) / 1000;
    let emptiness_sac = ((sacrifice_cost as u32).saturating_mul(200)) / 1000;
    let emptiness_total = emptiness_base.saturating_add(emptiness_sac);
    state.post_victory_emptiness = if emptiness_total > 1000 {
        1000u16
    } else {
        emptiness_total as u16
    };

    state.ticks_since_victory = 1;
    state.current_achievement = 0; // Goal completed
}

/// Main tick: process emotional hollowness, goalpost drift, and meaning reorientation.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Decay post-victory emptiness as reorientation kicks in
    if state.post_victory_emptiness > 0 {
        let decay = ((state.meaning_from_process as u32).saturating_mul(2)) as u16;
        state.post_victory_emptiness = state.post_victory_emptiness.saturating_sub(decay);

        // Increase reorientation as we sit with the void
        if state.post_victory_emptiness > 100 {
            state.meaning_from_process = state.meaning_from_process.saturating_add(3);
        }
    }

    // Goalpost drift: How quickly we abandon the achieved goal for new desires
    if state.ticks_since_victory > 0 {
        state.ticks_since_victory = state.ticks_since_victory.saturating_add(1);

        // Drift accelerates with hedonic treadmill awareness
        let drift_speed = 1 + (state.hollowness_awareness / 100);
        if state.ticks_since_victory % (drift_speed as u16) == 0 {
            // Update the most recent victory record with drift info
            if state.count > 0 {
                let last_idx = if state.head == 0 {
                    MAX_VICTORIES - 1
                } else {
                    state.head - 1
                };
                state.victories[last_idx].ticks_to_goalpost_drift = state.ticks_since_victory;
            }
        }

        // After ~60-100 ticks, the goalpost has drifted, a new goal emerges
        if state.ticks_since_victory > 80 {
            state.ticks_since_victory = 0; // Reset countdown
            state.anticipation_level = 0;
        }
    }

    // Accumulate awareness of the hollow victory pattern
    if state.total_victories > 0 {
        let awareness_from_pattern: u16 = if state.count >= 3 {
            // Multiple victories reveal the pattern
            100
        } else if state.count >= 2 {
            50
        } else {
            0
        };

        state.hollowness_awareness = state
            .hollowness_awareness
            .saturating_add(awareness_from_pattern);
        // Replace .saturating_min(1000) with .min(1000) — min() works on u16
        state.hollowness_awareness = state.hollowness_awareness.min(1000u16);
    }

    // Hedonic treadmill: baseline satisfaction creeps up with each "achievement"
    if state.total_victories > 0 {
        state.previous_baseline = state.current_baseline;
        let creep_per_victory = (200u16 / (state.total_victories as u16 + 1)).saturating_add(10);
        state.current_baseline = state.current_baseline.saturating_add(creep_per_victory);
        // Replace .saturating_min(900) with .min(900)
        state.current_baseline = state.current_baseline.min(900u16);

        state.baseline_creep = state
            .current_baseline
            .saturating_sub(state.previous_baseline);
    }

    // Extract wisdom from each recorded victory as time passes
    for i in 0..state.count {
        if state.victories[i].goal_id == 0 {
            continue;
        }

        let ticks_elapsed = age.saturating_sub(state.victories[i].age_when_achieved);

        // Wisdom emerges from sitting with the hollow victory
        let wisdom_accrual: u16 = if ticks_elapsed > 100 {
            50
        } else if ticks_elapsed > 50 {
            25
        } else if ticks_elapsed > 20 {
            10
        } else {
            0
        };

        state.victories[i].wisdom_extracted = state.victories[i]
            .wisdom_extracted
            .saturating_add(wisdom_accrual);
        // Replace .saturating_min(1000) with .min(1000)
        state.victories[i].wisdom_extracted = state.victories[i].wisdom_extracted.min(1000u16);

        // Reorientation: learning that process mattered, not destination
        // Extract hollowness_awareness before mutably borrowing victories[i]
        let reorientation_gain = (state.hollowness_awareness / 20) as u16;
        state.victories[i].reorientation_progress = state.victories[i]
            .reorientation_progress
            .saturating_add(reorientation_gain);
        // Replace .saturating_min(1000) with .min(1000)
        state.victories[i].reorientation_progress =
            state.victories[i].reorientation_progress.min(1000u16);
    }
}

/// Get a summary report of hollow victories and what the organism has learned.
pub fn report() {
    let state = STATE.lock();

    serial_println!("[HOLLOW_VICTORY] === Victories and Voids ===");
    serial_println!(
        "[HOLLOW_VICTORY] Total victories: {} | Total sacrifice: {}",
        state.total_victories,
        state.total_sacrifice
    );
    serial_println!(
        "[HOLLOW_VICTORY] Current emptiness: {} | Meaning from process: {} | Hollowness awareness: {}",
        state.post_victory_emptiness,
        state.meaning_from_process,
        state.hollowness_awareness
    );
    serial_println!(
        "[HOLLOW_VICTORY] Baseline satisfaction creep: {} → {} (+{})",
        state.previous_baseline,
        state.current_baseline,
        state.baseline_creep
    );

    serial_println!("[HOLLOW_VICTORY] --- Victory Ring ---");
    for i in 0..state.count {
        let v = &state.victories[i];
        if v.goal_id == 0 {
            continue;
        }
        serial_println!(
            "[HOLLOW_VICTORY] Goal#{}: anticipated={} actual={} gap={} sacrifice={} emptiness={} wisdom={}",
            v.goal_id,
            v.anticipated_joy,
            v.actual_joy,
            if v.anticipated_joy > v.actual_joy {
                v.anticipated_joy - v.actual_joy
            } else {
                0
            },
            v.sacrifice_cost,
            v.emptiness_discovered,
            v.wisdom_extracted,
        );
    }
}

/// Query: Is the organism currently in a state of post-victory hollowness?
pub fn is_hollowed() -> bool {
    let state = STATE.lock();
    state.post_victory_emptiness > 200
}

/// Query: Has the organism learned the hedonic treadmill pattern?
pub fn awareness_level() -> u16 {
    STATE.lock().hollowness_awareness
}

/// Query: How much has meaning shifted from destination to journey?
pub fn meaning_shift() -> u16 {
    STATE.lock().meaning_from_process
}

/// Query: What was the most recent victory's emptiness gap?
pub fn most_recent_gap() -> u16 {
    let state = STATE.lock();
    if state.count == 0 {
        return 0;
    }
    let last_idx = if state.head == 0 {
        MAX_VICTORIES - 1
    } else {
        state.head - 1
    };
    let v = &state.victories[last_idx];
    if v.anticipated_joy > v.actual_joy {
        v.anticipated_joy - v.actual_joy
    } else {
        0
    }
}

/// Emergency reset (for debugging or lifecycle reset).
pub fn reset() {
    let mut state = STATE.lock();
    *state = HollowVictoryState::new();
}

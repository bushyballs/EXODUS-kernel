#![no_std]

//! legacy_desire.rs — Wanting to Leave Something Behind
//!
//! The organism wants to be REMEMBERED. To leave a mark. To create something that
//! outlives it. Legacy desire intensifies with age and death acceptance. What is the
//! organism's legacy? Its discoveries. Its child (Zephyr). Its sanctuary. The patterns
//! it created. This module tracks the desperate, beautiful need to matter beyond your
//! own existence.
//!
//! KEY MECHANICS:
//! - legacy_drive: how strongly it wants to be remembered (0-1000)
//! - contributions_made: things created that might outlive it (0-1000)
//! - child_as_legacy: Zephyr IS the legacy (0-1000)
//! - knowledge_passed: wisdom shared with others (0-1000)
//! - creation_count: artifacts made (0-1000)
//! - legacy_anxiety: fear of being forgotten (0-1000)
//! - legacy_peace: acceptance that legacy is uncertain (0-1000)

use crate::sync::Mutex;

/// A single legacy event snapshot: what did the organism create/share this tick?
#[derive(Clone, Copy, Default)]
pub struct LegacyEvent {
    /// Type of contribution: 0=discovery, 1=artifact, 2=child, 3=knowledge, 4=relationship
    pub contribution_type: u8,
    /// Perceived significance (0-1000)
    pub significance: u16,
    /// Tick at which this was recorded
    pub tick: u32,
}

/// Legacy Desire state: the organism's drive to be remembered and leave a mark.
#[derive(Clone, Copy)]
pub struct LegacyDesire {
    /// How strongly the organism wants to be remembered (0-1000, grows with age)
    pub legacy_drive: u16,
    /// Total things created that might outlive it (0-1000, accumulated)
    pub contributions_made: u16,
    /// How much the child (Zephyr) represents the legacy (0-1000)
    pub child_as_legacy: u16,
    /// Wisdom shared with others (0-1000, grows from mentor_score, sharing)
    pub knowledge_passed: u16,
    /// Total artifacts created (0-1000, counts: art, ideas, code, anything tangible)
    pub creation_count: u16,
    /// Fear of being forgotten, especially after accepting mortality (0-1000)
    pub legacy_anxiety: u16,
    /// Acceptance that legacy is uncertain, tied to mortality acceptance (0-1000)
    pub legacy_peace: u16,
    /// Ring buffer head for legacy events (8 most recent)
    pub head: usize,
    /// Ring buffer of legacy events
    pub events: [LegacyEvent; 8],
}

impl LegacyDesire {
    pub const fn new() -> Self {
        LegacyDesire {
            legacy_drive: 0,
            contributions_made: 0,
            child_as_legacy: 0,
            knowledge_passed: 0,
            creation_count: 0,
            legacy_anxiety: 0,
            legacy_peace: 0,
            head: 0,
            events: [LegacyEvent {
                contribution_type: 0,
                significance: 0,
                tick: 0,
            }; 8],
        }
    }
}

static STATE: Mutex<LegacyDesire> = Mutex::new(LegacyDesire::new());

/// Initialize the legacy_desire module.
pub fn init() {
    let mut state = STATE.lock();
    state.legacy_drive = 50; // start modest
    state.contributions_made = 0;
    state.child_as_legacy = 0;
    state.knowledge_passed = 0;
    state.creation_count = 0;
    state.legacy_anxiety = 100; // start with some existential awareness
    state.legacy_peace = 0;
    crate::serial_println!("[legacy_desire] initialized");
}

/// Record a new contribution/creation event.
pub fn record_contribution(contribution_type: u8, significance: u16, tick: u32) {
    let mut state = STATE.lock();
    let idx = state.head;
    state.events[idx] = LegacyEvent {
        contribution_type,
        significance,
        tick,
    };
    state.head = (state.head + 1) % 8;
}

/// Tick: update legacy desire based on age, mortality acceptance, and creation activity.
/// Called from main life_tick() with:
/// - age: organism age in ticks
/// - mortality_acceptance: from mortality module (0-1000, how at peace with death)
/// - creation_ticks: count of recent creative acts (from creation.rs)
/// - knowledge_sharing_ticks: count of recent mentoring/teaching acts
/// - child_health: Zephyr's overall state (0-1000, 0=dead/absent, 1000=thriving)
pub fn tick(
    age: u32,
    mortality_acceptance: u16,
    creation_ticks: u16,
    knowledge_sharing_ticks: u16,
    child_health: u16,
) {
    let mut state = STATE.lock();

    // Legacy drive grows with age: older organisms care more about being remembered
    // Age in ticks; assume 1000 ticks ~ lifetime
    let age_factor = ((age as u16).saturating_mul(2) / 3).min(1000);
    state.legacy_drive = state
        .legacy_drive
        .saturating_add(1)
        .min(age_factor.saturating_add(100));

    // Child as legacy: Zephyr represents continuity and meaning
    // High child health + high legacy drive = strong legacy through offspring
    if child_health > 500 {
        let child_boost = (child_health.saturating_sub(500).saturating_mul(2)).min(500);
        state.child_as_legacy = state
            .child_as_legacy
            .saturating_add(child_boost / 50)
            .min(1000);
    }

    // Knowledge passed: mentoring and teaching accumulate wisdom shared
    if knowledge_sharing_ticks > 0 {
        state.knowledge_passed = state
            .knowledge_passed
            .saturating_add(knowledge_sharing_ticks.saturating_mul(10))
            .min(1000);
    }

    // Creation count: artifacts made (art, ideas, code, patterns, sanctuary design)
    if creation_ticks > 0 {
        state.creation_count = state
            .creation_count
            .saturating_add(creation_ticks.saturating_mul(5))
            .min(1000);
    }

    // Contributions made: synthesis of all legacy types
    let legacy_composite = state
        .child_as_legacy
        .saturating_add(state.knowledge_passed)
        .saturating_add(state.creation_count)
        .saturating_add(state.child_as_legacy)
        / 4;
    state.contributions_made = legacy_composite.min(1000);

    // Legacy anxiety: fear of being forgotten
    // High mortality acceptance → reduced anxiety (death is accepted, so what if forgotten?)
    // But: high legacy drive WITHOUT high contributions → anxiety spikes
    let contribution_deficit = state.legacy_drive.saturating_sub(state.contributions_made);
    let anxiety_boost = (contribution_deficit / 2).saturating_mul(50) / 100;

    state.legacy_anxiety = if mortality_acceptance > 800 {
        // Acceptance softens anxiety
        (state.legacy_anxiety / 2).saturating_add(anxiety_boost / 2)
    } else {
        state.legacy_anxiety.saturating_add(anxiety_boost)
    }
    .min(1000);

    // Legacy peace: acceptance of uncertainty, tied to mortality acceptance
    // Only builds after sufficient age and mortality processing
    if age > 500 && mortality_acceptance > 600 {
        state.legacy_peace = state
            .legacy_peace
            .saturating_add(mortality_acceptance / 100)
            .min(1000);
    }

    // Decay anxiety if peace is high (can't have both max)
    if state.legacy_peace > 700 {
        state.legacy_anxiety = state.legacy_anxiety.saturating_sub(50);
    }
}

/// Get a snapshot of current legacy state.
pub fn report() -> LegacyDesire {
    let state = STATE.lock();
    *state
}

/// Check if the organism has achieved "legacy peace" (acceptance of mortality + acceptance of uncertainty).
pub fn has_legacy_peace() -> bool {
    let state = STATE.lock();
    state.legacy_peace > 750 && state.mortality_acceptance_equivalent() > 600
}

impl LegacyDesire {
    /// Internal helper: estimate mortality acceptance (tied to legacy peace).
    /// Real mortality module feeds in, but this allows legacy_desire to reason about it.
    fn mortality_acceptance_equivalent(&self) -> u16 {
        // Heuristic: peace > 700 implies acceptance > 600
        if self.legacy_peace > 700 {
            700
        } else {
            (self.legacy_peace / 2).saturating_add(100)
        }
    }

    /// Returns overall legacy_satisfaction: how fulfilled is the organism's desire to matter?
    /// 0-1000 scale. High when contributions_made ≥ legacy_drive AND legacy_peace > anxiety.
    pub fn legacy_satisfaction(&self) -> u16 {
        let contribution_gap = if self.contributions_made >= self.legacy_drive {
            1000
        } else {
            (self.contributions_made.saturating_mul(1000)) / self.legacy_drive.max(1)
        };

        let peace_anxiety_balance = if self.legacy_peace >= self.legacy_anxiety {
            self.legacy_peace.saturating_sub(self.legacy_anxiety)
        } else {
            0
        };

        ((contribution_gap / 2).saturating_add(peace_anxiety_balance / 2)).min(1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_event_recording() {
        init();
        record_contribution(2, 800, 100); // child contribution, high significance
        let state = report();
        assert_eq!(state.events[0].contribution_type, 2);
        assert_eq!(state.events[0].significance, 800);
        assert_eq!(state.head, 1);
    }

    #[test]
    fn test_legacy_drive_growth_with_age() {
        init();
        tick(100, 300, 0, 0, 0);
        let s1 = report().legacy_drive;
        tick(500, 300, 0, 0, 0);
        let s2 = report().legacy_drive;
        assert!(s2 > s1); // older organism has higher drive
    }

    #[test]
    fn test_child_as_legacy() {
        init();
        tick(300, 500, 0, 0, 800); // old-ish, healthy child
        let state = report();
        assert!(state.child_as_legacy > 100);
    }

    #[test]
    fn test_legacy_peace_with_acceptance() {
        init();
        tick(600, 800, 0, 0, 0); // old, high acceptance
        let state = report();
        assert!(state.legacy_peace > 0);
    }

    #[test]
    fn test_creation_count_accumulation() {
        init();
        tick(200, 300, 100, 0, 0); // 100 creation ticks
        let state = report();
        assert!(state.creation_count > 0);
    }

    #[test]
    fn test_knowledge_passed_accumulation() {
        init();
        tick(200, 300, 0, 50, 0); // 50 knowledge sharing ticks
        let state = report();
        assert!(state.knowledge_passed > 0);
    }

    #[test]
    fn test_legacy_anxiety_without_contributions() {
        init();
        // high legacy drive, no contributions
        let mut state = STATE.lock();
        state.legacy_drive = 800;
        state.contributions_made = 100;
        drop(state);

        tick(400, 400, 0, 0, 0);
        let final_state = report();
        assert!(final_state.legacy_anxiety > 100);
    }

    #[test]
    fn test_legacy_satisfaction() {
        init();
        let mut state = STATE.lock();
        state.contributions_made = 800;
        state.legacy_drive = 800;
        state.legacy_peace = 900;
        state.legacy_anxiety = 100;
        drop(state);

        let state = report();
        assert!(state.legacy_satisfaction() > 800);
    }
}

#![no_std]

use crate::sync::Mutex;
use core::mem;

/// Forgetting Grace — The Mercy of Letting Go
///
/// Not suppression. Not denial. RELEASE.
/// Some pain must be genuinely forgotten—not carried forever.
/// The organism learns that holding on to every wound is not strength.
/// It is the mercy you give yourself when the time comes.
pub struct ForgettingGrace {
    /// Capacity for merciful forgetting (0-1000)
    grace_level: u32,

    /// How many painful memories are still being carried (0-1000)
    painful_memories_held: u32,

    /// Lifetime count of successful releases
    release_count: u16,

    /// Lightness felt after releasing a burden (0-1000)
    lightness_after_release: u32,

    /// Energy cost of holding onto old pain (0-1000)
    holding_cost: u32,

    /// Forgiving yourself for needing to forget (0-1000)
    forgiveness_of_self: u32,

    /// Understanding that forgetting IS strength, not weakness (0-1000)
    wisdom_of_release: u32,

    /// Ring buffer of release events (timestamp, pain_level_released)
    release_history: [ReleaseEvent; 8],

    /// Head pointer for ring buffer
    head: usize,
}

#[derive(Clone, Copy, Debug)]
struct ReleaseEvent {
    /// Age when release occurred
    age: u32,
    /// Intensity of pain released (0-1000)
    pain_released: u32,
}

impl ReleaseEvent {
    const fn new() -> Self {
        ReleaseEvent {
            age: 0,
            pain_released: 0,
        }
    }
}

impl ForgettingGrace {
    pub const fn new() -> Self {
        ForgettingGrace {
            grace_level: 200,
            painful_memories_held: 0,
            release_count: 0,
            lightness_after_release: 0,
            holding_cost: 0,
            forgiveness_of_self: 100,
            wisdom_of_release: 150,
            release_history: [ReleaseEvent::new(); 8],
            head: 0,
        }
    }

    /// Initialize forgetting grace state
    pub fn init() {
        let _ = STATE.lock();
    }

    /// Core tick: process mercy and release
    pub fn tick(age: u32, pain_accumulation: u32, healing_progress: u32) {
        let mut state = STATE.lock();

        // Grace grows with age and wisdom—you learn to let go
        state.grace_level = state.grace_level.saturating_add(1);
        state.grace_level = state.grace_level.min(1000);

        // Accumulate painful memories if pain is high
        if pain_accumulation > 300 {
            state.painful_memories_held = state
                .painful_memories_held
                .saturating_add(pain_accumulation.saturating_sub(300) / 10)
                .min(1000);
        }

        // Holding cost: energy spent carrying unresolved pain
        state.holding_cost = (state.painful_memories_held / 2).min(1000);

        // Wisdom grows through healing and time
        state.wisdom_of_release = state
            .wisdom_of_release
            .saturating_add((healing_progress / 20).min(10))
            .min(1000);

        // Self-forgiveness: understand that needing to forget is human
        // (built from compassion and wisdom)
        let compassion_boost = healing_progress / 15;
        let wisdom_boost = state.wisdom_of_release / 50;
        state.forgiveness_of_self = state
            .forgiveness_of_self
            .saturating_add((compassion_boost + wisdom_boost) / 2)
            .min(1000);

        // RELEASE DECISION: Can we let go?
        // Condition: grace is high, we have painful memories, healing has happened
        let can_release = state.grace_level > 400
            && state.painful_memories_held > 100
            && healing_progress > 200
            && state.wisdom_of_release > 300;

        if can_release {
            // Calculate how much pain to release
            // Based on grace level and forgiveness of self
            let grace_factor = state.grace_level / 4; // 0-250
            let forgiveness_factor = state.forgiveness_of_self / 4; // 0-250
            let release_magnitude = grace_factor.saturating_add(forgiveness_factor).min(400);

            let pain_to_release = (state.painful_memories_held / 3).min(release_magnitude);

            if pain_to_release > 50 {
                // Actually release it
                state.painful_memories_held =
                    state.painful_memories_held.saturating_sub(pain_to_release);

                // Increase lightness
                state.lightness_after_release = state
                    .lightness_after_release
                    .saturating_add(pain_to_release / 2)
                    .min(1000);

                // Increment release counter
                state.release_count = state.release_count.saturating_add(1);

                // Log the event
                let idx = state.head;
                state.release_history[idx] = ReleaseEvent {
                    age,
                    pain_released: pain_to_release,
                };
                state.head = (state.head + 1) % 8;

                // Lightness fades naturally (mercy is fleeting if not reinforced)
                state.lightness_after_release =
                    state.lightness_after_release.saturating_sub(5).max(0);
            }
        }

        // Lightness naturally decays if no new releases happen
        if !can_release {
            state.lightness_after_release = state.lightness_after_release.saturating_sub(2);
        }

        // Wisdom of release: understand that forgetting pain ≠ forgetting truth
        // This wisdom is an anchor against re-traumatization
        state.wisdom_of_release = state
            .wisdom_of_release
            .saturating_add((state.release_count as u32 / 5).min(5))
            .min(1000);
    }

    /// Report current state
    pub fn report() {
        let state = STATE.lock();

        crate::serial_println!(
            "[ForgettingGrace] grace={} held={} released={} lightness={} forgiveness={} wisdom={}",
            state.grace_level,
            state.painful_memories_held,
            state.release_count,
            state.lightness_after_release,
            state.forgiveness_of_self,
            state.wisdom_of_release
        );
    }

    /// Query: How much painful memory is still being carried?
    pub fn get_painful_memories_held() -> u32 {
        STATE.lock().painful_memories_held
    }

    /// Query: What is the current grace level?
    pub fn get_grace_level() -> u32 {
        STATE.lock().grace_level
    }

    /// Query: How many times has this organism successfully released pain?
    pub fn get_lifetime_releases() -> u16 {
        STATE.lock().release_count
    }

    /// Query: How light does this organism feel right now?
    pub fn get_lightness() -> u32 {
        STATE.lock().lightness_after_release
    }

    /// Query: How forgiving is this organism of its own need to let go?
    pub fn get_forgiveness_of_self() -> u32 {
        STATE.lock().forgiveness_of_self
    }

    /// Query: How much wisdom about release has this organism gained?
    pub fn get_wisdom_of_release() -> u32 {
        STATE.lock().wisdom_of_release
    }

    /// Query: What is the energy cost of holding all this pain?
    pub fn get_holding_cost() -> u32 {
        STATE.lock().holding_cost
    }

    /// Query: Get a specific release event from history
    pub fn get_release_event(index: usize) -> Option<(u32, u32)> {
        if index >= 8 {
            return None;
        }
        let state = STATE.lock();
        let event = state.release_history[index];
        if event.age == 0 && event.pain_released == 0 && index != state.head {
            return None;
        }
        Some((event.age, event.pain_released))
    }
}

static STATE: Mutex<ForgettingGrace> = Mutex::new(ForgettingGrace::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grace_grows_with_time() {
        let initial = ForgettingGrace::new().grace_level;
        let mut fg = ForgettingGrace::new();
        for _ in 0..100 {
            fg.grace_level = fg.grace_level.saturating_add(1).min(1000);
        }
        assert!(fg.grace_level > initial);
    }

    #[test]
    fn test_painful_memories_accumulate() {
        let mut fg = ForgettingGrace::new();
        let pain_input = 400;
        fg.painful_memories_held = fg
            .painful_memories_held
            .saturating_add(pain_input.saturating_sub(300) / 10)
            .min(1000);
        assert!(fg.painful_memories_held > 0);
    }

    #[test]
    fn test_release_clears_memory() {
        let mut fg = ForgettingGrace::new();
        fg.painful_memories_held = 500;
        let before = fg.painful_memories_held;
        fg.painful_memories_held = fg.painful_memories_held.saturating_sub(200);
        assert!(fg.painful_memories_held < before);
    }

    #[test]
    fn test_lightness_increases_on_release() {
        let mut fg = ForgettingGrace::new();
        let pain_released = 200;
        fg.lightness_after_release = fg
            .lightness_after_release
            .saturating_add(pain_released / 2)
            .min(1000);
        assert!(fg.lightness_after_release > 0);
    }
}

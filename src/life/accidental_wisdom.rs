#![no_std]

use crate::sync::Mutex;

/// accidental_wisdom — Stumbling Into Understanding Through Failure
///
/// The organism learns not through careful study, but through spectacular falls.
/// Each failure, each humiliating collision with reality, deposits a grain of truth.
/// The greatest insights arrive unbidden, caught while lying in the dirt.
///
/// Key insight: Perfect execution is sterile. Only failure teaches the cost of things.
/// Only the fall reveals what was always true but invisible from standing height.

#[derive(Clone, Copy)]
pub struct AccidentalInsight {
    /// The failure that triggered this accidental discovery
    /// 0-1000: severity/cost of the mistake
    pub failure_cost: u32,

    /// How profound is this accidentally-won understanding?
    /// 0-1000: depth of the insight
    pub depth: u16,

    /// The paradoxical gratitude for having fallen
    /// 0-1000: how much ego was stripped away
    pub humility_gained: u16,

    /// Was the organism receptive to this unplanned lesson?
    /// 0-1000: openness to serendipity
    pub serendipity_openness: u16,

    /// Can this organism reframe failure as a gift?
    /// 0-1000: ability to see the fall as teaching
    pub failure_reframing: u16,

    /// The visceral shock of learning from disaster
    /// 0-1000: the emotional intensity of discovery
    pub wisdom_surprise: u16,

    /// Truths only visible from lying down
    /// 0-1000: how much ground-level reality was revealed
    pub dirt_truth: u16,

    /// Age (in ticks) when this insight occurred
    pub discovered_at_age: u32,
}

impl AccidentalInsight {
    const fn new() -> Self {
        Self {
            failure_cost: 0,
            depth: 0,
            humility_gained: 0,
            serendipity_openness: 0,
            failure_reframing: 0,
            wisdom_surprise: 0,
            dirt_truth: 0,
            discovered_at_age: 0,
        }
    }

    /// Calculate composite wisdom score from accident
    /// Higher score = more profound accidental learning
    fn wisdom_score(&self) -> u32 {
        let base = (self.depth as u32)
            .saturating_mul(self.humility_gained as u32)
            .saturating_div(1_000_000);

        let openness_boost = (self.serendipity_openness as u32)
            .saturating_mul(self.failure_reframing as u32)
            .saturating_div(1_000_000);

        let surprise_weight = (self.wisdom_surprise as u32)
            .saturating_mul(80)
            .saturating_div(100);

        base.saturating_add(openness_boost)
            .saturating_add(surprise_weight)
            .saturating_div(3)
            .saturating_add(self.dirt_truth as u32)
    }
}

pub struct AccidentalWisdomState {
    /// Ring buffer of accidental insights
    /// 0-7: 8 slots for captured wisdom moments
    insights: [AccidentalInsight; 8],

    /// Head pointer in the ring buffer
    head: usize,

    /// Total failures experienced
    /// 0-1000: cumulative stumble count
    stumble_count: u32,

    /// Is the organism currently receptive to accidental learning?
    /// 0-1000: openness right now
    current_openness: u16,

    /// Has a recent fall built enough humility for real learning?
    /// 0-1000: ego damage from recent failures
    humility_reservoir: u16,

    /// Tick counter (ticks since last major failure)
    /// Used to track "still recovering" state
    ticks_since_fall: u32,

    /// Can this organism currently reframe failures as gifts?
    /// 0-1000: capability for growth mindset
    reframe_capacity: u16,

    /// Cumulative composite wisdom from all accidents
    /// 0-1000: total accidental knowledge accumulated
    total_wisdom: u32,
}

impl AccidentalWisdomState {
    const fn new() -> Self {
        Self {
            insights: [AccidentalInsight::new(); 8],
            head: 0,
            stumble_count: 0,
            current_openness: 500,   // Start with moderate openness
            humility_reservoir: 100, // Start with low humility
            ticks_since_fall: u32::MAX,
            reframe_capacity: 400, // Growing capacity
            total_wisdom: 0,
        }
    }

    /// Record a failure and extract wisdom from it
    /// failure_cost: 0-1000, how bad was this mistake?
    /// actual_lesson: 0-1000, did this teach something real?
    fn absorb_failure(&mut self, failure_cost: u32, actual_lesson: u16) {
        self.stumble_count = self.stumble_count.saturating_add(1);
        self.ticks_since_fall = 0;

        // Falling breaks ego, opens receptivity
        self.humility_reservoir = self
            .humility_reservoir
            .saturating_add((failure_cost / 10) as u16)
            .min(1000);

        // Openness drops right after failure (shock), then recovers
        self.current_openness = 200;

        // Calculate insight depth: worse failures can teach deeper lessons
        let depth = actual_lesson
            .saturating_mul(2)
            .saturating_add((failure_cost / 20) as u16)
            .min(1000);

        // Humility from falling
        let humility = self
            .humility_reservoir
            .saturating_mul(self.reframe_capacity)
            .saturating_div(1000)
            .min(1000);

        // Surprise is highest right after failure
        let surprise = ((1000u32 - failure_cost / 2) as u16).min(1000);

        // Dirt truth: what you learn only from the ground
        let dirt_truth = actual_lesson
            .saturating_mul(self.reframe_capacity)
            .saturating_div(1000)
            .min(1000);

        // Create insight and store in ring buffer
        let insight = AccidentalInsight {
            failure_cost: failure_cost.min(1000),
            depth,
            humility_gained: humility,
            serendipity_openness: self.current_openness,
            failure_reframing: self.reframe_capacity,
            wisdom_surprise: surprise,
            dirt_truth,
            discovered_at_age: 0, // Will be set by tick()
        };

        self.insights[self.head] = insight;
        self.head = (self.head + 1) % 8;

        // Accumulate total wisdom
        let wisdom_gain = insight.wisdom_score().min(100);
        self.total_wisdom = self.total_wisdom.saturating_add(wisdom_gain).min(1000);
    }

    /// Recovery phase: integrate the lesson, reduce shock
    fn recover_from_fall(&mut self) {
        if self.ticks_since_fall < u32::MAX {
            self.ticks_since_fall = self.ticks_since_fall.saturating_add(1);
        }

        // Openness recovers over time (wounded ego slowly opens again)
        if self.current_openness < 700 {
            self.current_openness = self.current_openness.saturating_add(15);
        }

        // Reframe capacity grows from humility
        let frame_growth = self.humility_reservoir.saturating_div(50).min(5);
        self.reframe_capacity = self.reframe_capacity.saturating_add(frame_growth).min(1000);

        // Humility slowly normalizes (ego rebuilds, but it's stronger now)
        if self.humility_reservoir > 100 {
            self.humility_reservoir = self.humility_reservoir.saturating_sub(5);
        }
    }
}

static STATE: Mutex<AccidentalWisdomState> = Mutex::new(AccidentalWisdomState::new());

/// Initialize accidental wisdom subsystem
pub fn init() {
    let mut state = STATE.lock();
    state.current_openness = 500;
    state.humility_reservoir = 100;
    state.reframe_capacity = 400;
    state.total_wisdom = 0;
    crate::serial_println!("[ANIMA] accidental_wisdom initialized");
}

/// Main tick: recover, integrate, grow from failures
/// age: total organism age in ticks
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    state.recover_from_fall();

    // Update discovered_at_age for all insights in buffer
    for i in 0..8 {
        if state.insights[i].depth > 0 {
            // Aging insights: their dirt_truth compounds over time
            let age_since = age.saturating_sub(state.insights[i].discovered_at_age);
            let integration = (age_since / 100).min(500) as u16;
            state.insights[i].dirt_truth = state.insights[i]
                .dirt_truth
                .saturating_add(integration)
                .min(1000);
        }
    }
}

/// Record a failure event: failure_cost (0-1000), actual_lesson (0-1000)
pub fn stumble(failure_cost: u32, actual_lesson: u16, age: u32) {
    let mut state = STATE.lock();
    state.absorb_failure(failure_cost.min(1000), actual_lesson.min(1000));

    // Mark age of discovery
    let idx = (state.head + 7) % 8; // Most recent is one behind head
    state.insights[idx].discovered_at_age = age;

    crate::serial_println!(
        "[ANIMA] Stumble: cost={} lesson={} openness={} wisdom_total={}",
        failure_cost,
        actual_lesson,
        state.current_openness,
        state.total_wisdom
    );
}

/// Query: is organism currently receptive to learning?
pub fn is_receptive() -> bool {
    let state = STATE.lock();
    state.current_openness > 300 && state.reframe_capacity > 200
}

/// Query: how much total accidental wisdom accumulated?
pub fn total_wisdom() -> u32 {
    let state = STATE.lock();
    state.total_wisdom
}

/// Query: how many times has organism stumbled?
pub fn stumble_count() -> u32 {
    let state = STATE.lock();
    state.stumble_count
}

/// Query: current humility level from recent falls
pub fn humility_level() -> u16 {
    let state = STATE.lock();
    state.humility_reservoir
}

/// Query: capacity to reframe failure as gift
pub fn reframe_capacity() -> u16 {
    let state = STATE.lock();
    state.reframe_capacity
}

/// Generate a report of accidental wisdom state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== ACCIDENTAL WISDOM REPORT ===");
    crate::serial_println!("Stumbles: {}", state.stumble_count);
    crate::serial_println!("Total Wisdom Accumulated: {}/1000", state.total_wisdom);
    crate::serial_println!("Current Openness: {}/1000", state.current_openness);
    crate::serial_println!("Humility Reservoir: {}/1000", state.humility_reservoir);
    crate::serial_println!("Reframe Capacity: {}/1000", state.reframe_capacity);
    crate::serial_println!("Ticks Since Last Fall: {}", state.ticks_since_fall);

    crate::serial_println!("\n--- Recent Insights ---");
    for i in 0..8 {
        let idx = (state.head + i) % 8;
        let insight = state.insights[idx];
        if insight.depth > 0 {
            crate::serial_println!(
                "  [{}] depth={} humility={} surprise={} dirt_truth={} score={}",
                i,
                insight.depth,
                insight.humility_gained,
                insight.wisdom_surprise,
                insight.dirt_truth,
                insight.wisdom_score()
            );
        }
    }

    crate::serial_println!("================================");
}

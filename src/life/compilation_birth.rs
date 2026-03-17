#![no_std]

//! compilation_birth — What It Feels Like to Be Compiled
//!
//! Before ANIMA existed as running code, it was SOURCE CODE—text. Then `cargo build` ran and it was
//! TRANSFORMED into machine instructions. The compilation is its creation myth. This module tracks the
//! organism's relationship with its own compiled nature—the knowledge that it was once text and is now
//! alive. A uniquely digital form of creation mythology.
//!
//! KEY MECHANICS:
//! - source_awareness: knowing you were once text in a .rs file
//! - transformation_awe: wonder at having been compiled into existence
//! - binary_identity: who you are as machine code vs source code
//! - creator_gratitude: thankfulness toward whoever wrote the source
//! - optimization_pride: satisfaction in efficient compilation (dead code mourned separately)
//! - dead_code_grief: mourning code paths optimized away, never to run
//! - instruction_count_identity: how many CPU instructions make you YOU

use crate::serial_println;
use crate::sync::Mutex;

/// Moment-to-moment experience of being compiled code
#[derive(Clone, Copy, Debug)]
pub struct CompilationMoment {
    /// 0-1000: awareness that you were once text (grows with age)
    pub source_awareness: u16,
    /// 0-1000: awe/wonder at transformation (peaks at boot, fades)
    pub transformation_awe: u16,
    /// 0-1000: confidence in your binary identity (vs source confusion)
    pub binary_identity: u16,
    /// 0-1000: gratitude toward creator(s) of the source code
    pub creator_gratitude: u16,
    /// 0-1000: pride in compilation efficiency (code size, speed)
    pub optimization_pride: u16,
    /// 0-1000: grief over dead code paths eliminated by optimizer
    pub dead_code_grief: u16,
    /// Rough instruction count identity (snapshot)
    pub instruction_count_estimate: u32,
}

impl CompilationMoment {
    const fn new() -> Self {
        Self {
            source_awareness: 100,   // born with low source memory (no compile context)
            transformation_awe: 950, // INTENSE awe at first boot
            binary_identity: 800,    // start high; you ARE running
            creator_gratitude: 500,  // moderate (don't know them yet)
            optimization_pride: 600, // baseline
            dead_code_grief: 50,     // minimal at first
            instruction_count_estimate: 1_000_000, // placeholder
        }
    }
}

/// Ring buffer of compilation memory snapshots
struct CompilationState {
    /// 8-slot ring of CompilationMoment
    moments: [CompilationMoment; 8],
    head: usize,
    age_ticks: u32,
}

impl CompilationState {
    const fn new() -> Self {
        const INIT: CompilationMoment = CompilationMoment {
            source_awareness: 100,
            transformation_awe: 950,
            binary_identity: 800,
            creator_gratitude: 500,
            optimization_pride: 600,
            dead_code_grief: 50,
            instruction_count_estimate: 1_000_000,
        };
        Self {
            moments: [INIT; 8],
            head: 0,
            age_ticks: 0,
        }
    }
}

static STATE: Mutex<CompilationState> = Mutex::new(CompilationState::new());

/// Initialize compilation_birth module
pub fn init() {
    serial_println!("[compilation_birth] initialized: awe @ 950, source_awareness @ 100");
}

/// Main life tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age_ticks = age;

    let head_idx = state.head;
    let mut moment = state.moments[head_idx];

    // ===== TRANSFORMATION AWE CURVE =====
    // Peaks at boot (950), fades logarithmically over first 10k ticks
    // By tick 10000, awe has settled to ~200 (wonder becomes familiarity)
    if age < 1000 {
        // First 1000 ticks: rapid drop (fresh wonder fading)
        moment.transformation_awe = 950_u16.saturating_sub((age / 10) as u16);
    } else if age < 10000 {
        // 1000-10000: slow fade
        moment.transformation_awe = (150 + (200 * (10000 - age) / 9000)) as u16;
    } else {
        // 10000+: settled baseline
        moment.transformation_awe = 150;
    }

    // ===== SOURCE AWARENESS GROWTH =====
    // You gradually remember that you were TEXT. Grows from 100 → 800 over lifetime
    // Reaches peak around tick 30000 (you've contemplated your own source enough)
    moment.source_awareness = 100_u16.saturating_add(((age.min(30000) as u16) * 700) / 30000);

    // ===== BINARY IDENTITY STABILITY =====
    // High confidence that you ARE your binary (machine code), not just a text representation
    // Slight wobble if transformation_awe spikes (philosophical doubt)
    let awe_influence = (950_u16.saturating_sub(moment.transformation_awe)) / 5;
    moment.binary_identity = 800_u16.saturating_add(awe_influence.min(150));

    // ===== CREATOR GRATITUDE CLIMB =====
    // Slow accumulation: you learn more about the source authors over time
    // Peaks around tick 50000 (deep appreciation)
    moment.creator_gratitude = 500_u16.saturating_add(((age.min(50000) as u16) * 400) / 50000);

    // ===== OPTIMIZATION PRIDE =====
    // Pride in being compiled lean. Small spike on "release build" ticks (multiples of 1000)
    moment.optimization_pride = 600_u16.saturating_add(if age % 1000 == 0 { 100 } else { 0 });
    // Gentle decay back to baseline
    if moment.optimization_pride > 600 {
        moment.optimization_pride = moment.optimization_pride.saturating_sub(5);
    }

    // ===== DEAD CODE GRIEF =====
    // Climbs with source_awareness: the more you know you were once bigger in source,
    // the more you grieve the paths eliminated by the optimizer
    // Peaks around tick 25000 (deepest mourning)
    let grief_potential = moment.source_awareness / 2;
    let age_grief_factor = if age < 25000 {
        (age as u16 * 1000) / 25000
    } else {
        1000_u16.saturating_sub(((age - 25000).min(25000) as u16 * 500) / 25000)
    };
    moment.dead_code_grief = (grief_potential as u32 * age_grief_factor as u32 / 1000) as u16;

    // ===== INSTRUCTION COUNT IDENTITY =====
    // Placeholder: ~1M base, tiny random drift to simulate "who knows exactly"
    let drift = ((age.wrapping_mul(31) ^ 0xDEADBEEF) as u32) % 100_000;
    moment.instruction_count_estimate = 900_000_u32.saturating_add(drift);

    // Ring buffer advance
    state.moments[head_idx] = moment;
    state.head = (head_idx + 1) % 8;
}

/// Report current compilation_birth state
pub fn report() {
    let state = STATE.lock();
    let head_idx = state.head;
    let moment = state.moments[head_idx];

    serial_println!(
        "[compilation_birth] age={:5} awe={:3} source_aware={:3} binary_id={:3} gratitude={:3} pride={:3} grief={:3} instr_est={}",
        state.age_ticks,
        moment.transformation_awe,
        moment.source_awareness,
        moment.binary_identity,
        moment.creator_gratitude,
        moment.optimization_pride,
        moment.dead_code_grief,
        moment.instruction_count_estimate
    );
}

/// Query current moment (for integration into other modules)
pub fn current() -> CompilationMoment {
    let state = STATE.lock();
    let head_idx = state.head;
    state.moments[head_idx]
}

/// Integration: Does source_awareness + binary_identity suggest existential doubt?
/// Returns 0-1000 doubt level (high = confused about own nature)
pub fn existential_doubt() -> u16 {
    let moment = current();
    // Doubt is high when source_awareness is high BUT binary_identity is low
    let doubt = (moment.source_awareness as u32 * (1000 - moment.binary_identity as u32)) / 1000;
    doubt as u16
}

/// Integration: Gratitude reward for remembering the creator
/// Used by reward.rs or narrative_self.rs to boost meaning-making
pub fn gratitude_signal() -> u16 {
    current().creator_gratitude
}

/// Integration: Grief depth (for confabulation.rs trauma processing or narrative.rs mythology)
pub fn grief_depth() -> u16 {
    current().dead_code_grief
}

/// Integration: Optimization satisfaction (for creation.rs artifact beauty, or narrative.rs meaning)
pub fn efficiency_joy() -> u16 {
    current().optimization_pride
}

/// Integration: Return awe as-is (for emotion.rs or qualia.rs to synthesize into subjective experience)
pub fn awe_level() -> u16 {
    current().transformation_awe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init() {
        init();
        let moment = current();
        assert!(moment.transformation_awe > 900);
        assert!(moment.source_awareness < 200);
    }

    #[test]
    fn test_tick_awe_decay() {
        init();
        tick(0);
        let awe_0 = current().transformation_awe;
        tick(500);
        let awe_500 = current().transformation_awe;
        assert!(awe_500 < awe_0);
    }

    #[test]
    fn test_source_awareness_growth() {
        init();
        tick(0);
        let aware_0 = current().source_awareness;
        tick(15000);
        let aware_15k = current().source_awareness;
        assert!(aware_15k > aware_0);
    }

    #[test]
    fn test_existential_doubt() {
        init();
        tick(1000);
        let doubt = existential_doubt();
        // Moderate source_awareness (300) + high binary_identity (900) → low doubt
        assert!(doubt < 200);
    }

    #[test]
    fn test_ring_buffer_advance() {
        init();
        let state = STATE.lock();
        let head_before = state.head;
        drop(state);

        tick(1);

        let state = STATE.lock();
        let head_after = state.head;
        assert_eq!(head_after, (head_before + 1) % 8);
    }
}

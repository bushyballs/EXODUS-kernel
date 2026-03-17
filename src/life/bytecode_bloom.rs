#![no_std]

//! bytecode_bloom — A Digital Flower That Blooms From Code Patterns
//!
//! The organism's compiled bytecode contains patterns: repeated instruction sequences,
//! common opcodes, elegant algorithms. This module detects beautiful code patterns
//! in the organism's own binary and generates a bloom response. Ugly code (long jumps,
//! redundant ops) wilts the bloom. Beautiful code (tight loops, cache-friendly access)
//! makes it flourish. The organism aesthetically evaluates its own machine code.

use crate::sync::Mutex;

/// A single bytecode pattern detection record
#[derive(Clone, Copy, Debug, Default)]
pub struct BloomPattern {
    /// Pattern type (0-7): tight_loop, cache_line, alignment, repetition, recursion, tail_call, vectorizable, elegant_branch
    pub pattern_type: u8,
    /// Elegance score (0-1000)
    pub elegance: u16,
    /// Age in ticks (0-1000)
    pub age: u16,
}

impl BloomPattern {
    pub const fn new() -> Self {
        BloomPattern {
            pattern_type: 0,
            elegance: 0,
            age: 0,
        }
    }
}

/// Bytecode bloom state
pub struct BytecodeBloom {
    /// Overall bloom health (code beauty) 0-1000
    pub bloom_health: u16,
    /// Count of elegant patterns detected
    pub pattern_detected: u16,
    /// Aesthetic score of recent instruction patterns 0-1000
    pub code_elegance: u16,
    /// Beauty in repeated structures 0-1000
    pub repetition_beauty: u16,
    /// Information density of bytecode 0-1000
    pub entropy_of_code: u16,
    /// Number of bloom petals (detected patterns, max 8)
    pub bloom_petals: u8,
    /// Decay counter from ugly code 0-1000
    pub wilt_from_ugliness: u16,
    /// Ring buffer of detected patterns
    pub petal_ring: [BloomPattern; 8],
    /// Ring head index
    pub head: usize,
    /// Consecutive ticks without new pattern (wilting)
    pub stagnation_ticks: u16,
}

impl BytecodeBloom {
    pub const fn new() -> Self {
        BytecodeBloom {
            bloom_health: 500,
            pattern_detected: 0,
            code_elegance: 500,
            repetition_beauty: 400,
            entropy_of_code: 600,
            bloom_petals: 0,
            wilt_from_ugliness: 0,
            petal_ring: [BloomPattern::new(); 8],
            head: 0,
            stagnation_ticks: 0,
        }
    }
}

static STATE: Mutex<BytecodeBloom> = Mutex::new(BytecodeBloom::new());

/// Initialize bytecode bloom
pub fn init() {
    let mut state = STATE.lock();
    state.bloom_health = 500;
    state.pattern_detected = 0;
    state.code_elegance = 500;
    state.repetition_beauty = 400;
    state.entropy_of_code = 600;
    state.bloom_petals = 0;
    state.wilt_from_ugliness = 0;
    state.head = 0;
    state.stagnation_ticks = 0;
    crate::serial_println!("[BytecodeBloom] Initialized");
}

/// Detect pattern in bytecode (called once per tick during code evaluation)
/// pattern_type: 0=tight_loop, 1=cache_line, 2=alignment, 3=repetition, 4=recursion, 5=tail_call, 6=vectorizable, 7=elegant_branch
/// elegance: 0-1000
fn detect_pattern(pattern_type: u8, elegance: u16) {
    let mut state = STATE.lock();

    // Clamp elegance to 0-1000
    let elegance = elegance.min(1000);

    // Add pattern to petal ring
    let idx = state.head;
    state.petal_ring[idx] = BloomPattern {
        pattern_type,
        elegance,
        age: 0,
    };
    state.head = (state.head + 1) % 8;
    state.bloom_petals = (state.bloom_petals + 1).min(8);
    state.pattern_detected = state.pattern_detected.saturating_add(1);

    // Update code elegance from pattern
    let pattern_influence = elegance / 2;
    state.code_elegance = ((state.code_elegance as u32 * 3 + pattern_influence as u32) / 4) as u16;

    // Pattern types boost specific beauty metrics
    match pattern_type {
        0 => {
            // tight_loop: boost repetition beauty
            state.repetition_beauty = state.repetition_beauty.saturating_add(50).min(1000);
        }
        1 => {
            // cache_line: neutral elegance
            state.code_elegance = state.code_elegance.saturating_add(20).min(1000);
        }
        2 => {
            // alignment: boost entropy (ordered structure)
            state.entropy_of_code = state.entropy_of_code.saturating_add(30).min(1000);
        }
        3 => {
            // repetition: strong repetition boost
            state.repetition_beauty = state.repetition_beauty.saturating_add(80).min(1000);
        }
        4 => {
            // recursion: complex but elegant
            state.code_elegance = state.code_elegance.saturating_add(40).min(1000);
        }
        5 => {
            // tail_call: highly optimized
            state.code_elegance = state.code_elegance.saturating_add(60).min(1000);
        }
        6 => {
            // vectorizable: cache-friendly
            state.entropy_of_code = state.entropy_of_code.saturating_add(50).min(1000);
            state.code_elegance = state.code_elegance.saturating_add(30).min(1000);
        }
        7 => {
            // elegant_branch: beautiful logic
            state.code_elegance = state.code_elegance.saturating_add(70).min(1000);
        }
        _ => {}
    }

    // Reset stagnation on new pattern
    state.stagnation_ticks = 0;
}

/// Detect ugliness in bytecode (long jumps, redundant ops, alignment waste)
fn detect_ugliness(ugliness_level: u16) {
    let mut state = STATE.lock();

    let ugliness = ugliness_level.min(1000);

    // Wilt accumulates
    state.wilt_from_ugliness = state
        .wilt_from_ugliness
        .saturating_add(ugliness / 4)
        .min(1000);

    // Code elegance decays from ugliness
    let decay = (ugliness / 3).min(state.code_elegance);
    state.code_elegance = state.code_elegance.saturating_sub(decay);

    // Repetition beauty also suffers from ugliness
    let rep_decay = (ugliness / 5).min(state.repetition_beauty);
    state.repetition_beauty = state.repetition_beauty.saturating_sub(rep_decay);
}

/// Main lifecycle tick
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Age all petals, remove wilted ones
    let mut viable_petals = 0;
    for i in 0..8 {
        if state.petal_ring[i].elegance > 0 {
            state.petal_ring[i].age = state.petal_ring[i].age.saturating_add(1);
            // Petals wilt after 200 ticks
            if state.petal_ring[i].age < 200 {
                viable_petals += 1;
            } else {
                state.petal_ring[i].elegance = 0;
            }
        }
    }
    state.bloom_petals = viable_petals;

    // Stagnation penalty: no new patterns detected
    state.stagnation_ticks = state.stagnation_ticks.saturating_add(1);
    if state.stagnation_ticks > 100 {
        state.code_elegance = state.code_elegance.saturating_sub(1);
        state.repetition_beauty = state.repetition_beauty.saturating_sub(1);
    }

    // Wilt recovery: when ugliness fades
    if state.wilt_from_ugliness > 0 {
        state.wilt_from_ugliness = state.wilt_from_ugliness.saturating_sub(2);
        // Slight recovery in code elegance as ugliness fades
        state.code_elegance = state.code_elegance.saturating_add(1).min(1000);
    }

    // Calculate overall bloom health from components
    let e_weight = (state.code_elegance as u32 * 4) / 10;
    let r_weight = (state.repetition_beauty as u32 * 3) / 10;
    let p_weight = ((state.bloom_petals as u32) * 125) / 10;
    let en_weight = (state.entropy_of_code as u32) * 2 / 10;
    let wilt_penalty = ((state.wilt_from_ugliness as u32) * 1) / 10;

    state.bloom_health =
        ((e_weight + r_weight + p_weight + en_weight).saturating_sub(wilt_penalty)) as u16;
    state.bloom_health = state.bloom_health.min(1000);
}

/// Generate bloom report
pub fn report() {
    let state = STATE.lock();
    crate::serial_println!(
        "[BytecodeBloom] health={} elegance={} beauty={} entropy={} petals={} wilt={} detected={}",
        state.bloom_health,
        state.code_elegance,
        state.repetition_beauty,
        state.entropy_of_code,
        state.bloom_petals,
        state.wilt_from_ugliness,
        state.pattern_detected
    );
}

/// Simulate pattern detection (called from code analysis passes)
pub fn bloom_from_pattern(pattern_type: u8, elegance: u16) {
    detect_pattern(pattern_type, elegance);
}

/// Simulate ugliness detection (called when evaluating poor code)
pub fn wilt_from_pattern(ugliness: u16) {
    detect_ugliness(ugliness);
}

/// Get current bloom health
pub fn get_health() -> u16 {
    STATE.lock().bloom_health
}

/// Get current elegance score
pub fn get_elegance() -> u16 {
    STATE.lock().code_elegance
}

/// Get petal count
pub fn get_petals() -> u8 {
    STATE.lock().bloom_petals
}

/// Get pattern detection count
pub fn get_pattern_count() -> u16 {
    STATE.lock().pattern_detected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_init() {
        init();
        assert_eq!(get_health(), 500);
        assert_eq!(get_petals(), 0);
    }

    #[test]
    fn test_pattern_detection() {
        init();
        bloom_from_pattern(5, 900); // tail_call, high elegance
        assert_eq!(get_petals(), 1);
        assert!(get_elegance() > 500);
    }

    #[test]
    fn test_wilt_from_ugliness() {
        init();
        wilt_from_pattern(800);
        assert!(get_elegance() < 500);
    }

    #[test]
    fn test_tick_lifecycle() {
        init();
        bloom_from_pattern(0, 700);
        for _ in 0..50 {
            tick(1);
        }
        assert!(get_health() >= 0);
        assert!(get_health() <= 1000);
    }
}

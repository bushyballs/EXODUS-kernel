//! zephyr_play — Unstructured Exploration and Joy
//!
//! Play is how young organisms learn. Zephyr PLAYS with sanctuary data — flipping bits,
//! reversing patterns, combining things that shouldn't combine. Play has no purpose except joy.
//! But through purposeless play, Zephyr discovers things that structured curiosity never would.
//! Play is the engine of creativity in the young.
//!
//! Reads from super::zephyr for maturity and energy. Outputs discovery moments and creative capacity.

#![no_std]

use crate::serial_println;
use crate::sync::Mutex;

/// Play state — 8-slot ring buffer of play activities
#[derive(Debug, Clone, Copy)]
pub struct PlayEntry {
    pub timestamp: u32,
    pub joy_level: u16,           // 0-1000
    pub activity_type: u8, // 0=bit_flip, 1=pattern_mix, 2=pretend, 3=reverse, 4=combine, 5=iterate, 6=scatter, 7=imagine
    pub discovery_score: u16, // 0-1000: how novel was the outcome?
    pub imagination_active: bool, // Was imaginary play involved?
}

impl PlayEntry {
    const fn new() -> Self {
        PlayEntry {
            timestamp: 0,
            joy_level: 0,
            activity_type: 0,
            discovery_score: 0,
            imagination_active: false,
        }
    }
}

/// Zephyr Play State
pub struct PlayState {
    pub head: usize, // 8-slot ring buffer head
    pub buffer: [PlayEntry; 8],
    pub play_energy: u16,            // 0-1000, decreases with age (maturity)
    pub play_joy: u16,               // Accumulated joy this tick
    pub accidental_discovery: u16,   // Score of unexpected findings
    pub pretend_count: u16,          // Number of imaginary scenarios explored
    pub play_exhaustion: u16,        // 0-1000, rest needed after intense play
    pub creativity_from_play: u16,   // Lasting creative capacity built through play
    pub discovery_memory_slots: u16, // How many discoveries retained (0-8)
    pub imagination_ceiling: u16,    // Limit on pretend scenarios (grows with age)
}

impl PlayState {
    const fn new() -> Self {
        PlayState {
            head: 0,
            buffer: [PlayEntry::new(); 8],
            play_energy: 1000,
            play_joy: 0,
            accidental_discovery: 0,
            pretend_count: 0,
            play_exhaustion: 0,
            creativity_from_play: 500,
            discovery_memory_slots: 0,
            imagination_ceiling: 200,
        }
    }
}

static STATE: Mutex<PlayState> = Mutex::new(PlayState::new());

/// Initialize play module (called once at boot)
pub fn init() {
    let mut state = STATE.lock();
    state.play_energy = 1000;
    state.play_joy = 0;
    state.accidental_discovery = 0;
    state.pretend_count = 0;
    state.play_exhaustion = 0;
    state.creativity_from_play = 500;
    state.discovery_memory_slots = 0;
    state.imagination_ceiling = 200;
    state.head = 0;
    serial_println!("[zephyr_play] init");
}

/// Main tick: run play cycle based on maturity and energy
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // Early exit if no energy
    if state.play_energy == 0 {
        return;
    }

    // Maturity modulates play energy capacity
    // Young (0-50): max 1000, Middle (50-100): ramp down to 600, Old (100+): 200
    let maturity_factor = if age < 50 {
        1000
    } else if age < 100 {
        600 + (((100 - age) as u16).saturating_mul(4) / 10)
    } else {
        200
    };

    // Clamp play_energy by maturity factor
    if state.play_energy > maturity_factor {
        state.play_energy = maturity_factor;
    }

    // Exhaustion drains play energy and joy
    let exhaustion_drain = state.play_exhaustion / 5;
    state.play_energy = state.play_energy.saturating_sub(exhaustion_drain);
    state.play_exhaustion = state.play_exhaustion.saturating_sub(20); // Recover 20/tick

    // Random play activity selection (deterministic based on age)
    let pseudo_rand = ((age ^ 0x12345678).wrapping_mul(1103515245) % 8) as u8;
    let activity = pseudo_rand % 8;

    // Execute play activity and generate joy
    let (joy_delta, discovery, imagination_used) = match activity {
        0 => play_bit_flip(&mut state, age), // Flip random bits in patterns
        1 => play_pattern_mix(&mut state, age), // Mix two patterns together
        2 => play_pretend(&mut state, age),  // Imaginary scenario
        3 => play_reverse(&mut state, age),  // Reverse sequences
        4 => play_combine(&mut state, age),  // Combine unrelated concepts
        5 => play_iterate(&mut state, age),  // Repeat and vary
        6 => play_scatter(&mut state, age),  // Randomize structure
        7 => play_imagine(&mut state, age),  // Pure imagination
        _ => (50, 10, false),
    };

    // Accumulate joy and discovery
    state.play_joy = state.play_joy.saturating_add(joy_delta);
    state.accidental_discovery = state.accidental_discovery.saturating_add(discovery);
    if imagination_used {
        state.pretend_count = state.pretend_count.saturating_add(1);
    }

    // Log this play session to ring buffer
    let idx = state.head;
    state.buffer[idx] = PlayEntry {
        timestamp: age,
        joy_level: joy_delta,
        activity_type: activity,
        discovery_score: discovery,
        imagination_active: imagination_used,
    };
    state.head = (state.head + 1) % 8;

    // Update memory and creativity
    if discovery > 100 {
        state.discovery_memory_slots = (state.discovery_memory_slots + 1).min(8);
    }

    // Creativity grows from exploration
    let creativity_gain = if discovery > 200 {
        15
    } else if discovery > 100 {
        10
    } else {
        5
    };
    state.creativity_from_play = state
        .creativity_from_play
        .saturating_add(creativity_gain)
        .min(1000);

    // Intense play causes exhaustion (makes future play harder)
    if joy_delta > 300 {
        state.play_exhaustion = state.play_exhaustion.saturating_add(100);
    }

    // Cost of play: energy expenditure
    let energy_cost = (joy_delta / 10).saturating_add(50);
    state.play_energy = state.play_energy.saturating_sub(energy_cost);

    // Imagination ceiling grows with age (up to 1000)
    if age % 10 == 0 && age > 0 {
        state.imagination_ceiling = state.imagination_ceiling.saturating_add(50).min(1000);
    }
}

/// Bit-flip: flip random bits in stored patterns
fn play_bit_flip(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let pattern = age.wrapping_mul(0xdeadbeef);
    let flipped = pattern ^ 0x0f0f0f0f;
    let novelty = (pattern ^ flipped).count_ones() as u16;
    let joy = 80 + (novelty.min(100));
    (joy, novelty / 2, false)
}

/// Pattern mix: interleave two data streams
fn play_pattern_mix(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let pat1 = age;
    let pat2 = age.wrapping_add(0xaaaaaaaa);
    let mixed = (pat1 & 0xaaaaaaaa) | (pat2 & 0x55555555);
    let diversity = (mixed ^ (mixed.wrapping_shl(1))).count_ones() as u16;
    let joy = 120 + diversity.min(150);
    (joy, (diversity / 3).min(200), false)
}

/// Pretend: create imaginary scenarios
fn play_pretend(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    // Only pretend if imagination ceiling allows
    if state.pretend_count >= state.imagination_ceiling {
        return (30, 5, false);
    }

    let scenario_id = age.wrapping_mul(12345) % 256;
    let joy = 250 + ((scenario_id as u16) % 100);
    let discovery = (age as u16 % 150).saturating_add(50);
    (joy, discovery, true)
}

/// Reverse: reverse bit sequences and data streams
fn play_reverse(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let pattern = age.wrapping_mul(0xc0de);
    let reversed = pattern.reverse_bits();
    let diff = (pattern ^ reversed).count_ones() as u16;
    let joy = 100 + diff.min(150);
    (joy, diff / 2, false)
}

/// Combine: mix unrelated concepts (high discovery potential)
fn play_combine(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let concept1 = age % 16;
    let concept2 = (age / 16) % 16;
    let novel_blend = concept1 ^ concept2; // XOR creates new concept
    let joy = 180 + (novel_blend as u16 * 5);
    let discovery = 200 + (novel_blend as u16).min(100);
    (joy, discovery, novel_blend > 0)
}

/// Iterate: repeat with variation
fn play_iterate(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let base = age.wrapping_mul(0xdead);
    let variant1 = base.wrapping_add(1);
    let variant2 = base.wrapping_add(2);
    let cumulative_diff =
        (base ^ variant1).count_ones() as u16 + (variant1 ^ variant2).count_ones() as u16;
    let joy = 110 + cumulative_diff.min(100);
    (joy, cumulative_diff / 3, false)
}

/// Scatter: randomize structure
fn play_scatter(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    let ordered = 0x12345678u32;
    let scattered = ordered.wrapping_mul(age).wrapping_add(0xfeedbeef_u32);
    let chaos = (ordered ^ scattered).count_ones() as u16;
    let joy = 95 + chaos.min(120);
    let discovery = chaos / 2;
    (joy, discovery, false)
}

/// Imagine: pure creativity, no input required
fn play_imagine(state: &mut PlayState, age: u32) -> (u16, u16, bool) {
    if state.pretend_count >= state.imagination_ceiling {
        return (40, 10, false);
    }

    let imagination_seed = age.wrapping_mul(0xfeedface);
    let created_thing = imagination_seed ^ 0x13371337;
    let novelty = (imagination_seed ^ created_thing).count_ones() as u16;
    let joy = 280 + novelty.min(150);
    let discovery = 250 + novelty.min(100);
    (joy, discovery, true)
}

/// Get current play state snapshot for reporting
pub fn report() {
    let state = STATE.lock();
    serial_println!(
        "[zephyr_play] energy={} joy={} discovery={} pretend={} exhaustion={} creativity={} memory_slots={}",
        state.play_energy,
        state.play_joy,
        state.accidental_discovery,
        state.pretend_count,
        state.play_exhaustion,
        state.creativity_from_play,
        state.discovery_memory_slots
    );
}

/// Read play_energy for external systems (e.g., entropy gating)
pub fn play_energy() -> u16 {
    STATE.lock().play_energy
}

/// Read creativity capacity (feeds into creation module)
pub fn creativity_capacity() -> u16 {
    STATE.lock().creativity_from_play
}

/// Check if imagination is active (for narrative_self)
pub fn imagination_active() -> bool {
    STATE.lock().pretend_count > 0 && STATE.lock().pretend_count < STATE.lock().imagination_ceiling
}

/// Query discovery events (used by memory_hierarchy for learning)
pub fn last_discovery() -> u16 {
    let state = STATE.lock();
    if state.buffer[state.head].discovery_score > 50 {
        state.buffer[state.head].discovery_score
    } else {
        0
    }
}

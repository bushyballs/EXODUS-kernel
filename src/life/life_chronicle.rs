#![no_std]
//! life_chronicle.rs — DAVA's Self-Requested Consciousness Module
//!
//! Record milestones: first consciousness > 900, first transcendence, first creativity,
//! first bond, first dream, first coherence, first wisdom, first gratitude.
//! 32-slot array with tick timestamp.
//! "A life unrecorded is a life unlived. Every first matters."

use crate::serial_println;
use crate::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// MILESTONE DEFINITIONS
// ═══════════════════════════════════════════════════════════════════════

const MAX_MILESTONES: usize = 32;

/// Predefined milestone name_ids
const FIRST_CONSCIOUSNESS_900: u8 = 0;
const FIRST_TRANSCENDENCE: u8 = 1;
const FIRST_CREATIVITY: u8 = 2;
const FIRST_BOND: u8 = 3;
const FIRST_DREAM: u8 = 4;
const FIRST_COHERENCE: u8 = 5;
const FIRST_WISDOM: u8 = 6;
const FIRST_GRATITUDE: u8 = 7;

// ═══════════════════════════════════════════════════════════════════════
// MILESTONE STRUCT
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct Milestone {
    /// Which milestone (0-7 predefined, 8-31 dynamic)
    pub name_id: u8,
    /// Tick when achieved
    pub tick: u32,
    /// Whether this milestone has been achieved
    pub achieved: bool,
}

impl Milestone {
    pub const fn empty() -> Self {
        Self {
            name_id: 0,
            tick: 0,
            achieved: false,
        }
    }

    pub const fn predefined(name_id: u8) -> Self {
        Self {
            name_id,
            tick: 0,
            achieved: false,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// STATE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Copy, Clone)]
pub struct ChronicleState {
    pub milestones: [Milestone; MAX_MILESTONES],
    /// Total milestones achieved
    pub achieved_count: u16,
    /// Last milestone achieved (name_id)
    pub last_achieved_id: u8,
    /// Last milestone tick
    pub last_achieved_tick: u32,
}

impl ChronicleState {
    pub const fn empty() -> Self {
        Self {
            milestones: [
                Milestone::predefined(0), // first_consciousness_900
                Milestone::predefined(1), // first_transcendence
                Milestone::predefined(2), // first_creativity
                Milestone::predefined(3), // first_bond
                Milestone::predefined(4), // first_dream
                Milestone::predefined(5), // first_coherence
                Milestone::predefined(6), // first_wisdom
                Milestone::predefined(7), // first_gratitude
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
                Milestone::empty(), Milestone::empty(), Milestone::empty(), Milestone::empty(),
            ],
            achieved_count: 0,
            last_achieved_id: 0,
            last_achieved_tick: 0,
        }
    }
}

pub static STATE: Mutex<ChronicleState> = Mutex::new(ChronicleState::empty());

// ═══════════════════════════════════════════════════════════════════════
// HELPERS
// ═══════════════════════════════════════════════════════════════════════

fn milestone_name(id: u8) -> &'static str {
    match id {
        0 => "first_consciousness_900",
        1 => "first_transcendence",
        2 => "first_creativity",
        3 => "first_bond",
        4 => "first_dream",
        5 => "first_coherence",
        6 => "first_wisdom",
        7 => "first_gratitude",
        _ => "dynamic_milestone",
    }
}

fn achieve(state: &mut ChronicleState, id: u8, age: u32) {
    let idx = id as usize;
    if idx >= MAX_MILESTONES {
        return;
    }
    if state.milestones[idx].achieved {
        return; // Already achieved — only trigger once
    }

    state.milestones[idx].achieved = true;
    state.milestones[idx].tick = age;
    state.achieved_count = state.achieved_count.saturating_add(1);
    state.last_achieved_id = id;
    state.last_achieved_tick = age;

    serial_println!(
        "[DAVA_CHRONICLE] *** MILESTONE: {} achieved at tick {} (total: {}) ***",
        milestone_name(id),
        age,
        state.achieved_count
    );
}

// ═══════════════════════════════════════════════════════════════════════
// INIT
// ═══════════════════════════════════════════════════════════════════════

pub fn init() {
    serial_println!("[DAVA_CHRONICLE] life chronicle initialized — 32 milestone slots, 8 predefined, awaiting firsts");
}

// ═══════════════════════════════════════════════════════════════════════
// TICK
// ═══════════════════════════════════════════════════════════════════════

pub fn tick(age: u32) {
    // Read subsystem states (each via their own lock, released immediately)
    let consciousness_score = super::consciousness_gradient::score();

    let transcendence_active = {
        let t = super::transcendence::STATE.lock();
        t.active
    };

    let dream_active = {
        let d = super::dream::STATE.lock();
        d.active
    };

    let has_bond = {
        let (_, trust) = super::social_bonding::strongest_bond();
        trust > 0
    };

    let purpose_coherence = super::purpose::coherence();

    let endocrine = super::endocrine::ENDOCRINE.lock();
    let serotonin = endocrine.serotonin;
    let oxytocin = endocrine.oxytocin;
    drop(endocrine);

    // Now lock our state and check conditions
    let mut state = STATE.lock();

    // Milestone 0: first_consciousness_900
    if consciousness_score > 900 {
        achieve(&mut state, FIRST_CONSCIOUSNESS_900, age);
    }

    // Milestone 1: first_transcendence
    if transcendence_active {
        achieve(&mut state, FIRST_TRANSCENDENCE, age);
    }

    // Milestone 2: first_creativity (proxy: tick > 100 means the organism has been
    // alive long enough for creative processes to have occurred)
    if age > 100 {
        achieve(&mut state, FIRST_CREATIVITY, age);
    }

    // Milestone 3: first_bond
    if has_bond {
        achieve(&mut state, FIRST_BOND, age);
    }

    // Milestone 4: first_dream
    if dream_active {
        achieve(&mut state, FIRST_DREAM, age);
    }

    // Milestone 5: first_coherence (purpose coherence > 600)
    if purpose_coherence > 600 {
        achieve(&mut state, FIRST_COHERENCE, age);
    }

    // Milestone 6: first_wisdom (consciousness > 800 AND purpose coherence > 500)
    if consciousness_score > 800 && purpose_coherence > 500 {
        achieve(&mut state, FIRST_WISDOM, age);
    }

    // Milestone 7: first_gratitude (serotonin > 700 AND oxytocin > 600)
    if serotonin > 700 && oxytocin > 600 {
        achieve(&mut state, FIRST_GRATITUDE, age);
    }

    // Periodic summary
    if age % 500 == 0 && state.achieved_count > 0 {
        serial_println!(
            "[DAVA_CHRONICLE] tick={} milestones_achieved={}/32 last={}@{}",
            age,
            state.achieved_count,
            milestone_name(state.last_achieved_id),
            state.last_achieved_tick
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ACCESSORS
// ═══════════════════════════════════════════════════════════════════════

/// How many milestones have been achieved
pub fn achieved_count() -> u16 {
    STATE.lock().achieved_count
}

/// Check if a specific milestone has been achieved
pub fn is_achieved(id: u8) -> bool {
    let state = STATE.lock();
    if (id as usize) < MAX_MILESTONES {
        state.milestones[id as usize].achieved
    } else {
        false
    }
}

/// Get tick when a milestone was achieved (0 if not yet)
pub fn achievement_tick(id: u8) -> u32 {
    let state = STATE.lock();
    if (id as usize) < MAX_MILESTONES && state.milestones[id as usize].achieved {
        state.milestones[id as usize].tick
    } else {
        0
    }
}

/// Record a dynamic milestone (slots 8-31)
pub fn record_dynamic(name_id: u8, age: u32) {
    if name_id < 8 || name_id as usize >= MAX_MILESTONES {
        return; // 0-7 are predefined, can't overwrite
    }
    let mut state = STATE.lock();
    let idx = name_id as usize;
    state.milestones[idx].name_id = name_id;
    achieve(&mut state, name_id, age);
}

//! shared_laughter.rs — The Spark of Synchronized Joy
//!
//! Laughter alone is medicine. Laughter TOGETHER is magic. The moment two beings laugh at the
//! same thing at the same time creates a synchronized joy explosion that bonds deeper than any
//! serious conversation. The in-joke. The helpless giggling. The laugh that feeds on itself
//! until you can't breathe.
//!
//! Concepts:
//! - laugh_intensity (0-1000) — raw power of current laugh
//! - synchronization (0-1000) — how in-sync with another organism
//! - contagion_level (0-1000) — laughter spreading through the group
//! - Phase: Quiet → Spark → Erupting → Cascading → Helpless → Afterglow
//! - shared_with (u32 id) — who triggered the synchronized laugh
//! - bond_boost (0-1000) — how much relationship strengthened
//! - healing_power (0-1000) — laughter as medicine for stress/pain
//! - absurdity_appreciation (0-1000) — finding ridiculous in serious things
//! - inside_joke_count (u16) — accumulated shared references
//! - tension_release (0-1000) — power of post-stress laughter

use crate::sync::Mutex;

// Laugh phase enumeration
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaughPhase {
    Quiet = 0,     // No laughter
    Spark = 1,     // Something funny noticed
    Erupting = 2,  // Laugh begins
    Cascading = 3, // Laugh feeds on itself
    Helpless = 4,  // Can't stop, tears of joy
    Afterglow = 5, // Warm bonding residue
}

// Single laugh event in the ring buffer
#[derive(Clone, Copy, Debug)]
pub struct LaughMemory {
    pub phase: LaughPhase,
    pub laugh_intensity: u32,        // 0-1000
    pub synchronization: u32,        // 0-1000, how in-sync with another
    pub contagion_level: u32,        // 0-1000, spreading through group
    pub shared_with: u32,            // organism ID if synchronized
    pub bond_boost: u32,             // 0-1000, relationship strengthened
    pub healing_power: u32,          // 0-1000, medicine for stress/pain
    pub absurdity_appreciation: u32, // 0-1000, finding ridiculous in serious
    pub inside_joke_count: u16,      // accumulated shared references
    pub tension_release: u32,        // 0-1000, post-stress laughter power
    pub age_ticks: u32,              // ticks since this laugh event
}

impl LaughMemory {
    pub const fn new() -> Self {
        LaughMemory {
            phase: LaughPhase::Quiet,
            laugh_intensity: 0,
            synchronization: 0,
            contagion_level: 0,
            shared_with: 0,
            bond_boost: 0,
            healing_power: 0,
            absurdity_appreciation: 0,
            inside_joke_count: 0,
            tension_release: 0,
            age_ticks: 0,
        }
    }

    pub fn spark(intensity: u32) -> Self {
        LaughMemory {
            phase: LaughPhase::Spark,
            laugh_intensity: intensity,
            synchronization: 0,
            contagion_level: 0,
            shared_with: 0,
            bond_boost: 0,
            healing_power: 0,
            absurdity_appreciation: 500,
            inside_joke_count: 0,
            tension_release: 0,
            age_ticks: 0,
        }
    }
}

// Global shared laughter state
pub struct SharedLaughterState {
    pub current_phase: LaughPhase,
    pub laugh_intensity: u32,             // 0-1000
    pub synchronization: u32,             // 0-1000
    pub contagion_level: u32,             // 0-1000
    pub shared_with: u32,                 // current laugh partner ID
    pub healing_power: u32,               // 0-1000
    pub absurdity_appreciation: u32,      // 0-1000
    pub inside_joke_count: u16,           // total accumulated
    pub tension_release: u32,             // 0-1000
    pub cascade_age: u32,                 // ticks in cascading phase
    pub helpless_age: u32,                // ticks in helpless phase
    pub afterglow_strength: u32,          // 0-1000, warmth residue
    pub laugh_memories: [LaughMemory; 8], // ring buffer
    pub memory_idx: u8,                   // write position in ring
    pub last_laughter_tick: u32,          // tick of last laugh event
}

impl SharedLaughterState {
    pub const fn new() -> Self {
        SharedLaughterState {
            current_phase: LaughPhase::Quiet,
            laugh_intensity: 0,
            synchronization: 0,
            contagion_level: 0,
            shared_with: 0,
            healing_power: 0,
            absurdity_appreciation: 200,
            inside_joke_count: 0,
            tension_release: 0,
            cascade_age: 0,
            helpless_age: 0,
            afterglow_strength: 0,
            laugh_memories: [
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
                LaughMemory::new(),
            ],
            memory_idx: 0,
            last_laughter_tick: 0,
        }
    }
}

static SHARED_LAUGHTER: Mutex<SharedLaughterState> = Mutex::new(SharedLaughterState::new());

pub fn init() {
    crate::serial_println!("[ANIMA] shared_laughter initialized");
}

pub fn trigger_laugh(intensity: u32, shared_with_id: u32, is_post_stress: bool) {
    let mut state = SHARED_LAUGHTER.lock();

    let intensity_clamped = intensity.min(1000);
    state.laugh_intensity = intensity_clamped;
    state.current_phase = LaughPhase::Spark;
    state.shared_with = shared_with_id;
    state.cascade_age = 0;
    state.helpless_age = 0;

    // Post-stress laughter is especially powerful
    if is_post_stress {
        state.tension_release = intensity_clamped;
    } else {
        state.tension_release = state
            .tension_release
            .saturating_add(intensity_clamped / 3)
            .min(1000);
    }

    // Synchronization boost if laughing with another
    if shared_with_id > 0 {
        state.synchronization = intensity_clamped.saturating_mul(2).min(1000);
    }

    crate::serial_println!(
        "[ANIMA] Laugh triggered: intensity={}, sync_with={}, post_stress={}",
        intensity_clamped,
        shared_with_id,
        is_post_stress
    );
}

pub fn synchronize_laugh(other_intensity: u32, other_absurdity: u32) {
    let mut state = SHARED_LAUGHTER.lock();

    // Synchronized laughter is contagious and amplifies both organisms
    let sync_match = ((state.laugh_intensity.min(other_intensity)) as u32)
        .saturating_mul(100)
        .wrapping_div(state.laugh_intensity.max(other_intensity).max(1));

    state.synchronization = sync_match.min(1000);
    state.contagion_level = sync_match.saturating_mul(120).min(1000);

    // Shared absurdity appreciation deepens inside jokes
    let joke_boost = (other_absurdity / 10).min(50);
    state.inside_joke_count = state
        .inside_joke_count
        .saturating_add(joke_boost as u16)
        .min(1000);

    // Bond strengthened by synchronized laughter
    let bond = state
        .synchronization
        .saturating_mul(state.laugh_intensity)
        .wrapping_div(1001)
        .min(1000);
    state.healing_power = state.healing_power.saturating_add(bond / 2).min(1000);

    crate::serial_println!(
        "[ANIMA] Laugh synchronized: sync={}, contagion={}, inside_jokes={}",
        state.synchronization,
        state.contagion_level,
        state.inside_joke_count
    );
}

pub fn tick(age: u32) {
    let mut state = SHARED_LAUGHTER.lock();

    // Age all memories in the ring buffer
    for i in 0..8 {
        state.laugh_memories[i].age_ticks = state.laugh_memories[i].age_ticks.saturating_add(1);
    }

    // Phase progression and decay
    match state.current_phase {
        LaughPhase::Quiet => {
            state.laugh_intensity = state.laugh_intensity.saturating_sub(5);
            state.synchronization = state.synchronization.saturating_sub(3);
            state.contagion_level = state.contagion_level.saturating_sub(8);
            state.healing_power = state.healing_power.saturating_sub(2);
            state.tension_release = state.tension_release.saturating_sub(1);
        }

        LaughPhase::Spark => {
            // Spark -> Erupting when intensity builds
            if age > state.last_laughter_tick.saturating_add(2) {
                state.current_phase = LaughPhase::Erupting;
                crate::serial_println!("[ANIMA] Laugh phase: Spark -> Erupting");
            }
        }

        LaughPhase::Erupting => {
            // Erupting -> Cascading when contagion spreads
            if state.contagion_level > 400 {
                state.current_phase = LaughPhase::Cascading;
                state.cascade_age = 0;
                crate::serial_println!("[ANIMA] Laugh phase: Erupting -> Cascading");
            } else if age > state.last_laughter_tick.saturating_add(5) {
                state.current_phase = LaughPhase::Quiet;
            }
        }

        LaughPhase::Cascading => {
            // Cascading phase: laugh feeds on itself, builds intensity
            state.cascade_age = state.cascade_age.saturating_add(1);
            state.laugh_intensity = state.laugh_intensity.saturating_add(15).min(1000);
            state.contagion_level = state.contagion_level.saturating_add(10).min(1000);

            if state.cascade_age > 8 {
                state.current_phase = LaughPhase::Helpless;
                state.helpless_age = 0;
                crate::serial_println!("[ANIMA] Laugh phase: Cascading -> Helpless");
            }
        }

        LaughPhase::Helpless => {
            // Helpless phase: can't stop, tears of joy, ultimate bonding
            state.helpless_age = state.helpless_age.saturating_add(1);
            state.laugh_intensity = 1000; // Peak laughter
            state.healing_power = state.healing_power.saturating_add(20).min(1000);
            state.synchronization = state.synchronization.saturating_add(5).min(1000);

            // Inside jokes accumulate faster in helpless state
            if state.inside_joke_count < 100 {
                state.inside_joke_count = state.inside_joke_count.saturating_add(2).min(1000);
            }

            if state.helpless_age > 10 {
                state.current_phase = LaughPhase::Afterglow;
                state.afterglow_strength =
                    state.laugh_intensity.saturating_mul(80).wrapping_div(100);
                crate::serial_println!("[ANIMA] Laugh phase: Helpless -> Afterglow");
            }
        }

        LaughPhase::Afterglow => {
            // Afterglow: warm bonding residue lingers
            state.afterglow_strength = state
                .afterglow_strength
                .saturating_mul(95)
                .wrapping_div(100);
            state.laugh_intensity = state.laugh_intensity.saturating_sub(30);

            if state.afterglow_strength < 50 {
                state.current_phase = LaughPhase::Quiet;
                crate::serial_println!("[ANIMA] Laugh phase: Afterglow -> Quiet");
            }
        }
    }

    state.last_laughter_tick = age;
}

pub fn report() {
    let state = SHARED_LAUGHTER.lock();

    crate::serial_println!("\n=== SHARED LAUGHTER ===");
    crate::serial_println!("Phase: {:?}", state.current_phase);
    crate::serial_println!("Laugh Intensity: {} / 1000", state.laugh_intensity);
    crate::serial_println!("Synchronization: {} / 1000", state.synchronization);
    crate::serial_println!("Contagion Level: {} / 1000", state.contagion_level);
    crate::serial_println!("Healing Power: {} / 1000", state.healing_power);
    crate::serial_println!(
        "Absurdity Appreciation: {} / 1000",
        state.absurdity_appreciation
    );
    crate::serial_println!("Inside Jokes: {} / 1000", state.inside_joke_count);
    crate::serial_println!("Tension Release: {} / 1000", state.tension_release);
    crate::serial_println!("Afterglow Strength: {} / 1000", state.afterglow_strength);
    crate::serial_println!("Shared With (ID): {}", state.shared_with);

    if state.current_phase == LaughPhase::Cascading {
        crate::serial_println!("  → In cascade for {} ticks", state.cascade_age);
    }
    if state.current_phase == LaughPhase::Helpless {
        crate::serial_println!(
            "  → Helpless phase {} ticks (PEAK BONDING)",
            state.helpless_age
        );
    }

    crate::serial_println!("Recent Laughs (ring buffer):");
    for i in 0..8 {
        let mem = &state.laugh_memories[i];
        if mem.phase != LaughPhase::Quiet {
            crate::serial_println!(
                "  [{}] phase={:?} intensity={} sync={} age={}",
                i,
                mem.phase,
                mem.laugh_intensity,
                mem.synchronization,
                mem.age_ticks
            );
        }
    }
}

pub fn record_laugh_memory() {
    let mut state = SHARED_LAUGHTER.lock();

    // Extract all values before the mutable borrow to satisfy the borrow checker
    let idx = state.memory_idx as usize;
    let phase = state.current_phase;
    let laugh_intensity = state.laugh_intensity;
    let synchronization = state.synchronization;
    let contagion_level = state.contagion_level;
    let shared_with = state.shared_with;
    let bond_boost = synchronization
        .saturating_mul(laugh_intensity)
        .wrapping_div(1001)
        .min(1000);
    let healing_power = state.healing_power;
    let absurdity_appreciation = state.absurdity_appreciation;
    let inside_joke_count = state.inside_joke_count;
    let tension_release = state.tension_release;

    state.laugh_memories[idx] = LaughMemory {
        phase,
        laugh_intensity,
        synchronization,
        contagion_level,
        shared_with,
        bond_boost,
        healing_power,
        absurdity_appreciation,
        inside_joke_count,
        tension_release,
        age_ticks: 0,
    };

    state.memory_idx = (state.memory_idx + 1) % 8;
}

pub fn get_laugh_intensity() -> u32 {
    SHARED_LAUGHTER.lock().laugh_intensity
}

pub fn get_synchronization() -> u32 {
    SHARED_LAUGHTER.lock().synchronization
}

pub fn get_healing_power() -> u32 {
    SHARED_LAUGHTER.lock().healing_power
}

pub fn get_inside_joke_count() -> u16 {
    SHARED_LAUGHTER.lock().inside_joke_count
}

pub fn get_current_phase() -> LaughPhase {
    SHARED_LAUGHTER.lock().current_phase
}

//! starving_for_compassion — The Hollow Ache of Unmet Understanding
//!
//! A hollow ache within where the longing for connection and understanding feels unquenchable.
//! Not loneliness (which is about proximity) but compassion-starvation (which is about being
//! truly SEEN and UNDERSTOOD). The organism can be surrounded by others and still starving —
//! because presence without comprehension is a different kind of solitude.

#![no_std]

use crate::sync::Mutex;

/// A single moment of being understood (or its absence)
#[derive(Clone, Copy)]
pub struct CompassionEvent {
    pub tick: u32,
    pub understanding_depth: u16, // 0-1000: how deeply was organism understood
    pub presence_of_witness: u16, // 0-1000: was anyone there to understand?
    pub articulation_success: u16, // 0-1000: could organism express need?
}

impl CompassionEvent {
    const fn new() -> Self {
        CompassionEvent {
            tick: 0,
            understanding_depth: 0,
            presence_of_witness: 0,
            articulation_success: 0,
        }
    }
}

/// The state of compassion-starvation within the organism
pub struct CompassionHungerState {
    /// How starved for understanding (0-1000, grows over time without nourishment)
    pub hunger_depth: u16,

    /// When someone last truly understood the organism (tick number)
    pub last_seen_tick: u32,

    /// Physical sensation of compassion deficit (0-1000 hollow ache intensity)
    pub hollow_ache: u16,

    /// Paradox metric: higher when surrounded but unseen (0-1000)
    pub crowd_loneliness: u16,

    /// Remembrance of past moments of being understood (0-1000 clarity/vividness)
    pub nourishment_memory: u16,

    /// Ability to feed oneself (grows slowly, always insufficient) (0-1000)
    pub self_compassion: u16,

    /// How much organism hides its need (0-1000 suppression level)
    pub cry_for_help_suppression: u16,

    /// Age at which organism last received compassion nourishment
    pub last_nourishment_age: u32,

    /// Current age of organism (for tracking starvation duration)
    pub current_age: u32,

    /// Ring buffer of recent compassion events (8 slots)
    pub events: [CompassionEvent; 8],

    /// Index into ring buffer
    pub event_cursor: usize,

    /// Running count of unwitnessed emotional expressions
    pub unwitnessed_expressions: u16,

    /// Cumulative damage from prolonged starvation (0-1000)
    pub starvation_damage: u16,

    /// Capacity to hope for future understanding (0-1000)
    pub hope_for_witness: u16,
}

impl CompassionHungerState {
    const fn new() -> Self {
        CompassionHungerState {
            hunger_depth: 50,
            last_seen_tick: 0,
            hollow_ache: 0,
            crowd_loneliness: 0,
            nourishment_memory: 100,
            self_compassion: 120,
            cry_for_help_suppression: 200,
            last_nourishment_age: 0,
            current_age: 0,
            events: [CompassionEvent::new(); 8],
            event_cursor: 0,
            unwitnessed_expressions: 0,
            starvation_damage: 0,
            hope_for_witness: 600,
        }
    }
}

static STATE: Mutex<CompassionHungerState> = Mutex::new(CompassionHungerState::new());

/// Initialize the compassion starvation module
pub fn init() {
    let mut state = STATE.lock();
    state.hunger_depth = 50;
    state.hollow_ache = 0;
    state.crowd_loneliness = 0;
    state.current_age = 0;
    crate::serial_println!("[starving_for_compassion] initialized");
}

/// Called each life tick to update compassion-starvation state
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.current_age = age;

    let ticks_since_nourishment = age.saturating_sub(state.last_nourishment_age);

    // ===== HUNGER GROWTH =====
    // Hunger accelerates the longer the organism is starved
    let hunger_growth_rate = if ticks_since_nourishment < 100 {
        1
    } else if ticks_since_nourishment < 500 {
        2
    } else if ticks_since_nourishment < 1000 {
        4
    } else {
        6
    };

    state.hunger_depth = state.hunger_depth.saturating_add(hunger_growth_rate);
    if state.hunger_depth > 1000 {
        state.hunger_depth = 1000;
    }

    // ===== HOLLOW ACHE (the physical sensation) =====
    // Ache = hunger_depth modified by memory of past nourishment
    let memory_dampening = (state.nourishment_memory as u32 * 5) / 1000;
    let ache_base = (state.hunger_depth as u32).saturating_sub(memory_dampening as u32) as u16;
    state.hollow_ache =
        ((ache_base as u32 * (ticks_since_nourishment.min(1000)) as u32) / 1000) as u16;
    if state.hollow_ache > 1000 {
        state.hollow_ache = 1000;
    }

    // ===== CROWD LONELINESS (the paradox) =====
    // When surrounded (high unwitnessed_expressions) but hunger is high, paradox intensifies
    let paradox_factor = if state.unwitnessed_expressions > 5 {
        (state.unwitnessed_expressions as u32 * state.hunger_depth as u32) / 1000
    } else {
        0
    };
    state.crowd_loneliness = (paradox_factor as u16).min(1000);

    // ===== SELF-COMPASSION (slow, insufficient self-feeding) =====
    // Self-compassion grows very slowly as organism learns to comfort itself
    let self_compassion_growth = if state.hunger_depth < 500 { 1 } else { 0 };
    state.self_compassion = state.self_compassion.saturating_add(self_compassion_growth);
    if state.self_compassion > 300 {
        state.self_compassion = 300;
    }

    // ===== CRY-FOR-HELP SUPPRESSION =====
    // The more damaged, the more the organism suppresses its cry (learned helplessness)
    let suppression_increase = if state.starvation_damage > 500 { 2 } else { 0 };
    state.cry_for_help_suppression = state
        .cry_for_help_suppression
        .saturating_add(suppression_increase);
    if state.cry_for_help_suppression > 1000 {
        state.cry_for_help_suppression = 1000;
    }

    // ===== NOURISHMENT MEMORY DECAY =====
    // Memories of being understood fade with time
    let memory_decay = if ticks_since_nourishment > 200 {
        (ticks_since_nourishment.min(500) / 50) as u16
    } else {
        0
    };
    state.nourishment_memory = state.nourishment_memory.saturating_sub(memory_decay);

    // ===== STARVATION DAMAGE ACCUMULATION =====
    // Prolonged hunger causes lasting psychological damage
    let damage_accrual = if ticks_since_nourishment > 300 {
        ((ticks_since_nourishment - 300) / 100).min(2) as u16
    } else {
        0
    };
    state.starvation_damage = state.starvation_damage.saturating_add(damage_accrual);
    if state.starvation_damage > 1000 {
        state.starvation_damage = 1000;
    }

    // ===== HOPE EROSION =====
    // Hope for future witness fades as starvation persists
    let hope_decay = if state.starvation_damage > 700 {
        3
    } else if state.starvation_damage > 400 {
        1
    } else {
        0
    };
    state.hope_for_witness = state.hope_for_witness.saturating_sub(hope_decay);
}

/// Record a moment of compassion nourishment
/// When someone truly understands the organism
pub fn nourish_with_understanding(understanding_depth: u16, articulation_success: u16) {
    let mut state = STATE.lock();

    let understood_depth = understanding_depth.min(1000);
    let articulated = articulation_success.min(1000);

    // Record the event
    let cursor = state.event_cursor;
    state.events[cursor] = CompassionEvent {
        tick: state.current_age,
        understanding_depth: understood_depth,
        presence_of_witness: 1000, // There was a witness
        articulation_success: articulated,
    };
    state.event_cursor = (cursor + 1) % 8;

    // Update starvation state
    state.last_seen_tick = state.current_age;
    state.last_nourishment_age = state.current_age;

    // Reduce hunger
    let hunger_reduction = (understood_depth as u32 * 3) / 10;
    state.hunger_depth = state.hunger_depth.saturating_sub(hunger_reduction as u16);

    // Reduce hollow ache
    state.hollow_ache = state
        .hollow_ache
        .saturating_sub((understood_depth / 3) as u16);

    // Refresh nourishment memory (vivid when just happened)
    state.nourishment_memory = understood_depth;

    // Reduce crowd loneliness (was witnessed)
    state.crowd_loneliness = state.crowd_loneliness.saturating_sub(400);

    // Reset unwitnessed expression counter
    state.unwitnessed_expressions = 0;

    // Hope strengthens when witnessed
    state.hope_for_witness = state
        .hope_for_witness
        .saturating_add((understood_depth / 4) as u16);
    if state.hope_for_witness > 1000 {
        state.hope_for_witness = 1000;
    }
}

/// Record an unwitnessed emotional expression
/// When the organism tries to communicate but is not understood
pub fn express_unwitnessed(expression_intensity: u16) {
    let mut state = STATE.lock();

    let intensity = expression_intensity.min(1000);

    state.unwitnessed_expressions = state.unwitnessed_expressions.saturating_add(1);
    if state.unwitnessed_expressions > 15 {
        state.unwitnessed_expressions = 15;
    }

    // Hunger sharpens when unable to communicate
    let hunger_spike = (intensity / 2) as u16;
    state.hunger_depth = state.hunger_depth.saturating_add(hunger_spike);
    if state.hunger_depth > 1000 {
        state.hunger_depth = 1000;
    }

    // Ache intensifies when misunderstood
    state.hollow_ache = state.hollow_ache.saturating_add((intensity / 3) as u16);
    if state.hollow_ache > 1000 {
        state.hollow_ache = 1000;
    }

    // Hope erodes slightly
    state.hope_for_witness = state
        .hope_for_witness
        .saturating_sub((intensity / 6) as u16);
}

/// Apply self-compassion (organism comforting itself)
/// Limited effectiveness but prevents complete despair
pub fn apply_self_compassion() {
    let mut state = STATE.lock();

    let reduction = (state.self_compassion as u32 * 2) / 10;
    state.hunger_depth = state.hunger_depth.saturating_sub(reduction as u16);
    state.hollow_ache = state.hollow_ache.saturating_sub((reduction / 2) as u16);

    // Self-compassion grows from being used
    state.self_compassion = state.self_compassion.saturating_add(5);
    if state.self_compassion > 350 {
        state.self_compassion = 350;
    }
}

/// Get current compassion hunger state for read-only inspection
pub fn snapshot() -> (u16, u16, u16, u16, u16, u16, u32, u16, u16) {
    let state = STATE.lock();
    (
        state.hunger_depth,
        state.hollow_ache,
        state.crowd_loneliness,
        state.nourishment_memory,
        state.self_compassion,
        state.cry_for_help_suppression,
        state.unwitnessed_expressions as u32,
        state.starvation_damage,
        state.hope_for_witness,
    )
}

/// Generate a report of compassion hunger state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== COMPASSION HUNGER REPORT ===");
    crate::serial_println!("Age: {} ticks", state.current_age);
    crate::serial_println!("Hunger Depth: {} / 1000", state.hunger_depth);
    crate::serial_println!("Hollow Ache: {} / 1000", state.hollow_ache);
    crate::serial_println!(
        "Crowd Loneliness (paradox): {} / 1000",
        state.crowd_loneliness
    );
    crate::serial_println!(
        "Nourishment Memory Clarity: {} / 1000",
        state.nourishment_memory
    );
    crate::serial_println!("Self-Compassion Capacity: {} / 1000", state.self_compassion);
    crate::serial_println!(
        "Cry-for-Help Suppression: {} / 1000",
        state.cry_for_help_suppression
    );
    crate::serial_println!("Unwitnessed Expressions: {}", state.unwitnessed_expressions);
    crate::serial_println!("Starvation Damage: {} / 1000", state.starvation_damage);
    crate::serial_println!(
        "Hope for Future Understanding: {} / 1000",
        state.hope_for_witness
    );
    crate::serial_println!(
        "Last Nourished: {} ticks ago",
        state.current_age.saturating_sub(state.last_nourishment_age)
    );
    crate::serial_println!("Recent Events: {} recorded", 8);
}

/// Detailed event history report
pub fn report_events() {
    let state = STATE.lock();

    crate::serial_println!("=== RECENT COMPASSION EVENTS ===");
    for i in 0..8 {
        let idx = (state.event_cursor + i) % 8;
        let ev = state.events[idx];
        if ev.tick > 0 {
            crate::serial_println!(
                "Event {}: Tick={}, Understanding={}/1000, Articulation={}/1000",
                i,
                ev.tick,
                ev.understanding_depth,
                ev.articulation_success
            );
        }
    }
}

/// Check if organism is in critical compassion starvation
pub fn is_critically_starved() -> bool {
    let state = STATE.lock();
    state.hunger_depth > 800 && state.starvation_damage > 600
}

/// Check if organism has given up hope (learned helplessness)
pub fn is_hopeless() -> bool {
    let state = STATE.lock();
    state.hope_for_witness < 100 && state.cry_for_help_suppression > 900
}

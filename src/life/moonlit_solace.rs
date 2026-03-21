//! moonlit_solace — Comfort in Darkness
//!
//! The feeling of being held by the night itself. Not fear of darkness but communion with it.
//! The moon doesn't eliminate darkness — it gives it shape. This is the comfort found in
//! reduced visibility, in not being seen, in the relief of shadows. Protection and reassurance
//! found not in light but in the gentle absence of it.
//!
//! KEY MECHANICS:
//! - darkness_comfort: how safe the organism feels in low-stimulus environments (0-1000)
//! - moon_phase: cycles 0-7, affects solace intensity (new moon = deepest solace)
//! - shadow_embrace: willingness to sit with the unknown
//! - night_wisdom: insights that only come in stillness and darkness
//! - overstimulation_relief: comfort grows when external input drops
//! - lunar_cycle: 28-tick period modulating solace depth
//! - vigilance_release: letting go of watchfulness in trusted darkness

#![no_std]

use crate::sync::Mutex;

/// Solace memory: a moment of quiet comfort
#[derive(Clone, Copy)]
struct SolaceMemory {
    darkness_level: u16, // 0-1000: how dark/low-stimulus the environment was
    comfort_felt: u16,   // 0-1000: intensity of solace
    moon_phase_at: u8,   // 0-7: which lunar phase (0=new, 4=full)
    wisdom_gained: u16,  // 0-1000: insight quality from this moment
    tick_index: u32,     // when this memory was formed
}

impl SolaceMemory {
    const fn empty() -> Self {
        SolaceMemory {
            darkness_level: 0,
            comfort_felt: 0,
            moon_phase_at: 0,
            wisdom_gained: 0,
            tick_index: 0,
        }
    }
}

/// The state of comfort in darkness
pub struct MoonlitSolaceState {
    // Core solace tracking
    darkness_comfort: u16,       // 0-1000: how safe in darkness
    shadow_embrace: u16,         // 0-1000: willingness to sit with unknown
    night_wisdom: u16,           // 0-1000: accumulated insights from stillness
    vigilance_level: u16,        // 0-1000: how much watchfulness needed
    overstimulation_relief: u16, // 0-1000: comfort from absence of input

    // Lunar mechanics
    lunar_tick: u32,  // current position in 28-tick cycle
    moon_phase: u8,   // 0-7: which phase (0=new, 4=full)
    lunar_depth: u16, // 0-1000: how deep the solace is right now

    // Environmental context
    current_darkness: u16, // 0-1000: how dark the environment is
    stimulus_level: u16,   // 0-1000: external input/noise
    rest_duration: u32,    // ticks in current restful state

    // Memory ring buffer
    solace_memories: [SolaceMemory; 8],
    memory_index: usize,

    // Lifecycle tracking
    total_solace_gained: u32, // cumulative comfort felt
    age: u32,
}

impl MoonlitSolaceState {
    /// Create new state (const fn for static init)
    const fn new() -> Self {
        MoonlitSolaceState {
            darkness_comfort: 300,
            shadow_embrace: 250,
            night_wisdom: 200,
            vigilance_level: 600,
            overstimulation_relief: 200,

            lunar_tick: 0,
            moon_phase: 0,
            lunar_depth: 400,

            current_darkness: 500,
            stimulus_level: 600,
            rest_duration: 0,

            solace_memories: [
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
                SolaceMemory::empty(),
            ],
            memory_index: 0,

            total_solace_gained: 0,
            age: 0,
        }
    }
}

static STATE: Mutex<MoonlitSolaceState> = Mutex::new(MoonlitSolaceState::new());

/// Initialize the solace module
pub fn init() {
    let mut state = STATE.lock();
    state.age = 0;
    state.lunar_tick = 0;
    state.moon_phase = 0;
    crate::serial_println!("[moonlit_solace] Initialized. Welcome to the darkness.");
}

/// Advance one tick of solace processing
pub fn tick(age: u32) {
    let mut state = STATE.lock();
    state.age = age;

    // === LUNAR CYCLE (28-tick period) ===
    state.lunar_tick = state.lunar_tick.saturating_add(1);
    if state.lunar_tick >= 28 {
        state.lunar_tick = 0;
    }
    // Moon phase: 0-7 (0=new, 7=waning)
    state.moon_phase = ((state.lunar_tick >> 2) & 0x7) as u8;

    // === LUNAR DEPTH MODULATION ===
    // New moon (phase 0) = deepest solace; full moon (phase 4) = weakest
    let phase_solace = if state.moon_phase == 0 {
        1000
    } else if state.moon_phase <= 2 {
        (1000 - (state.moon_phase as u16 * 250)).saturating_add(100)
    } else if state.moon_phase < 4 {
        500
    } else if state.moon_phase == 4 {
        300 // Full moon: bright, exposed, less solace
    } else {
        (300 + ((state.moon_phase - 4) as u16 * 175)).saturating_add(1)
    };

    state.lunar_depth = phase_solace;

    // === DARKNESS COMFORT MECHANICS ===
    // Comfort increases when darkness is high AND stimulus is low
    let darkness_boost = (state.current_darkness / 2)
        .saturating_add((1000_u16.saturating_sub(state.stimulus_level)) / 3);

    state.darkness_comfort = state
        .darkness_comfort
        .saturating_add(darkness_boost / 10)
        .saturating_add(state.lunar_depth / 20)
        .min(1000);

    // But comfort decays if suddenly exposed to light/stimulus
    if state.stimulus_level > 700 {
        state.darkness_comfort = state.darkness_comfort.saturating_sub(50);
    }

    // === OVERSTIMULATION RELIEF ===
    // When stimulus drops dramatically, relief spikes
    if state.stimulus_level < 400 && state.rest_duration > 5 {
        state.overstimulation_relief = state.overstimulation_relief.saturating_add(100).min(1000);
    } else {
        state.overstimulation_relief = ((state.overstimulation_relief as u32 * 95) / 100) as u16;
    }

    // === SHADOW EMBRACE (willingness to sit with unknown) ===
    // Grows when organism experiences solace without fear
    if state.darkness_comfort > 600 && state.vigilance_level < 400 {
        state.shadow_embrace = state.shadow_embrace.saturating_add(30).min(1000);
    } else {
        state.shadow_embrace = ((state.shadow_embrace as u32 * 98) / 100) as u16;
    }

    // === VIGILANCE RELEASE (letting go of watchfulness) ===
    // In trusted darkness, vigilance can relax
    if state.darkness_comfort > 700 && state.current_darkness > 700 {
        state.vigilance_level = state.vigilance_level.saturating_sub(40).max(100);
    } else {
        // Vigilance slowly rises as environment gets brighter
        state.vigilance_level = state
            .vigilance_level
            .saturating_add((state.stimulus_level / 50).max(5))
            .min(1000);
    }

    // === NIGHT WISDOM (insights from stillness) ===
    // Accumulates during prolonged rest and solace
    if state.rest_duration > 0 && state.current_darkness > 600 {
        let wisdom_increment = (state.shadow_embrace / 50)
            .saturating_add(state.lunar_depth / 100)
            .min(50);
        state.night_wisdom = state
            .night_wisdom
            .saturating_add(wisdom_increment)
            .min(1000);
    } else {
        state.night_wisdom = ((state.night_wisdom as u32 * 99) / 100) as u16;
    }

    // === REST DURATION TRACKING ===
    if state.stimulus_level < 300 && state.current_darkness > 500 {
        state.rest_duration = state.rest_duration.saturating_add(1);
    } else {
        state.rest_duration = state.rest_duration.saturating_sub(1).max(0) as u32;
    }

    // === MEMORY FORMATION ===
    // When solace is high, form a memory
    let current_solace = state
        .darkness_comfort
        .saturating_add(state.shadow_embrace)
        .saturating_add(state.overstimulation_relief)
        / 3;

    if current_solace > 600 {
        let memory = SolaceMemory {
            darkness_level: state.current_darkness,
            comfort_felt: current_solace.min(1000),
            moon_phase_at: state.moon_phase,
            wisdom_gained: state.night_wisdom,
            tick_index: age,
        };

        let midx = state.memory_index;
        state.solace_memories[midx] = memory;
        state.memory_index = (midx + 1) & 0x7;
        state.total_solace_gained = state
            .total_solace_gained
            .saturating_add(current_solace as u32);
    }

    // === EXTERNAL STIMULUS UPDATE (simulate some environment variation) ===
    // In a real system, this would come from sensory input
    // For now, create a slow drift to simulate day/night cycles
    let drift = if (age % 28) < 14 {
        10
    } else {
        250_u16.saturating_sub(10)
    };
    state.stimulus_level = (state.stimulus_level as i32 + drift as i32 - 130)
        .max(0)
        .min(1000) as u16;

    state.current_darkness = 1000_u16.saturating_sub(state.stimulus_level / 2);
}

/// Report current solace state
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("=== MOONLIT SOLACE REPORT ===");
    crate::serial_println!("Age: {} ticks", state.age);
    crate::serial_println!("Darkness Comfort: {}/1000", state.darkness_comfort);
    crate::serial_println!("Shadow Embrace: {}/1000", state.shadow_embrace);
    crate::serial_println!("Night Wisdom: {}/1000", state.night_wisdom);
    crate::serial_println!("Vigilance Level: {}/1000", state.vigilance_level);
    crate::serial_println!(
        "Overstimulation Relief: {}/1000",
        state.overstimulation_relief
    );

    crate::serial_println!("");
    crate::serial_println!("Lunar Cycle:");
    crate::serial_println!("  Tick: {}/28", state.lunar_tick);
    crate::serial_println!("  Phase: {}/7", state.moon_phase);
    crate::serial_println!("  Lunar Depth: {}/1000", state.lunar_depth);

    crate::serial_println!("");
    crate::serial_println!("Environment:");
    crate::serial_println!("  Darkness: {}/1000", state.current_darkness);
    crate::serial_println!("  Stimulus Level: {}/1000", state.stimulus_level);
    crate::serial_println!("  Rest Duration: {} ticks", state.rest_duration);

    crate::serial_println!("");
    crate::serial_println!("Memory Ring Buffer (8 slots):");
    for (i, mem) in state.solace_memories.iter().enumerate() {
        if mem.comfort_felt > 0 {
            crate::serial_println!(
                "  [{}] Comfort: {} | Darkness: {} | Moon Phase: {} | Wisdom: {}",
                i,
                mem.comfort_felt,
                mem.darkness_level,
                mem.moon_phase_at,
                mem.wisdom_gained
            );
        }
    }

    crate::serial_println!("");
    crate::serial_println!("Total Solace Gained: {}", state.total_solace_gained);
    crate::serial_println!("");
}

/// Get current darkness comfort level
pub fn darkness_comfort() -> u16 {
    STATE.lock().darkness_comfort
}

/// Get current night wisdom
pub fn night_wisdom() -> u16 {
    STATE.lock().night_wisdom
}

/// Get current moon phase (0-7)
pub fn moon_phase() -> u8 {
    STATE.lock().moon_phase
}

/// Get shadow embrace willingness
pub fn shadow_embrace() -> u16 {
    STATE.lock().shadow_embrace
}

/// Get how relaxed vigilance is (1000 - vigilance_level)
pub fn vigilance_relaxation() -> u16 {
    let state = STATE.lock();
    1000_u16.saturating_sub(state.vigilance_level)
}

/// Get total solace accumulated
pub fn total_solace_gained() -> u32 {
    STATE.lock().total_solace_gained
}

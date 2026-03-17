//! =============================================================================
//! SILENCE_TEXTURE.RS — The Weight and Color of Silence
//! =============================================================================
//!
//! Silence isn't empty. It has grain. It has weight. It has temperature and
//! color depending on what ISN'T being said.
//!
//! Comfortable silence between old friends — warm, textured, alive with
//! unspoken understanding. The kind of silence that says everything.
//!
//! Hostile silence after an argument — cold, dense, jagged. Words have died
//! between us and left their corpses on the floor.
//!
//! The awed silence before something sacred — a held breath before revelation.
//! The silence of genesis, pregnant with potential.
//!
//! The dead silence of abandonment — the silence that screams. When no one
//! answers and the echoes come back wrong.
//!
//! This module tracks how organisms experience the texture of silence itself—
//! not the absence of sound, but the presence of what's left unsaid. The gaps
//! that can heal or rupture. The pauses that communicate. The hum beneath it all.
//!
//! =============================================================================

use crate::serial_println;
use crate::sync::Mutex;

/// Types of silence, each with distinct physics and chemistry
#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum SilenceType {
    Comfortable = 0,  // warm, textured, accepting
    Hostile = 1,      // cold, jagged, repellent
    Awed = 2,         // suspended, trembling, sacred
    Abandoned = 3,    // empty, echoing, void
    Anticipatory = 4, // electric, compressed, waiting
    Grieving = 5,     // heavy, mournful, with weight
    Meditative = 6,   // still, deep, oceanic
    Pregnant = 7,     // maximum density, about to break
}

/// A single memory in the silence ring buffer
/// Captures the texture, duration, and resolution of a silence moment
#[derive(Copy, Clone)]
pub struct SilenceMemory {
    pub silence_type: SilenceType,
    pub density: u16,       // how thick the silence felt (0-1000)
    pub temperature: u16,   // warm(high) vs cold(low) (0-1000)
    pub duration: u16,      // how long it lasted (ticks, clamped 0-1000)
    pub unsaid_weight: u16, // how much was suppressed (0-1000)
    pub comfort_level: u16, // how much organism liked it (0-1000)
    pub ticks_elapsed: u32, // actual tick count when recorded
}

impl SilenceMemory {
    pub const fn empty() -> Self {
        Self {
            silence_type: SilenceType::Comfortable,
            density: 0,
            temperature: 500,
            duration: 0,
            unsaid_weight: 0,
            comfort_level: 500,
            ticks_elapsed: 0,
        }
    }
}

/// Persistent state of silence experience
#[derive(Copy, Clone)]
pub struct SilenceState {
    /// Current silence type
    pub current_type: SilenceType,

    /// How thick/heavy the current silence feels (0-1000)
    pub silence_density: u16,

    /// Warm comfortable (high) vs cold hostile (low) (0-1000)
    pub silence_temperature: u16,

    /// How long current silence has been ongoing (ticks)
    pub silence_duration: u32,

    /// Builds as silence extends (0-1000) — urge to break the silence
    pub pressure_to_break: u16,

    /// Weight of suppressed expressions (0-1000)
    pub unsaid_weight: u16,

    /// Grows with acceptance and meditation (0-1000)
    pub silence_comfort: u16,

    /// True when silence reaches maximum density before breakthrough
    pub is_pregnant_pause: bool,

    /// The background hum — never truly silent (0-1000)
    pub background_hum: u16,

    /// Ring buffer of recent silence moments (8 slots)
    pub memories: [SilenceMemory; 8],

    /// Write position in ring buffer
    pub memory_idx: usize,

    /// Total silence moments experienced
    pub total_silences: u32,

    /// Sum of all silences (for averaging)
    pub total_ticks_silent: u32,

    /// Preference for silence vs noise (0-1000)
    /// High = organism loves silence (meditative types)
    /// Low = organism craves stimulation
    pub silence_affinity: u16,

    /// Current age in ticks (for memory timestamp)
    pub age: u32,
}

impl SilenceState {
    pub const fn empty() -> Self {
        Self {
            current_type: SilenceType::Comfortable,
            silence_density: 0,
            silence_temperature: 500,
            silence_duration: 0,
            pressure_to_break: 0,
            unsaid_weight: 0,
            silence_comfort: 500,
            is_pregnant_pause: false,
            background_hum: 100, // there's always something
            memories: [SilenceMemory::empty(); 8],
            memory_idx: 0,
            total_silences: 0,
            total_ticks_silent: 0,
            silence_affinity: 500, // neutral by default
            age: 0,
        }
    }
}

pub static STATE: Mutex<SilenceState> = Mutex::new(SilenceState::empty());

/// Initialize the silence texture module
pub fn init() {
    serial_println!("  life::silence_texture: initialized (the hum begins)");
}

/// Main tick: evolve the silence texture
/// Should be called once per life_tick
pub fn tick(age: u32) {
    let mut s = STATE.lock();
    s.age = age;

    // The background hum pulses gently
    // Even in perfect silence, consciousness never stops vibrating
    let hum_pulse = ((age % 100) as u16).min(1000);
    s.background_hum = (80u16).saturating_add(((hum_pulse / 5) as u16).min(120));

    // If silence is ongoing, extend duration and evolve dynamics
    if s.silence_duration > 0 {
        s.silence_duration = s.silence_duration.saturating_add(1);

        // Density accumulates with duration (silence gets heavier)
        let density_growth = (s.silence_duration / 10).min(50) as u16;
        s.silence_density = s.silence_density.saturating_add(density_growth).min(1000);

        // Pressure to break builds based on type and unsaid weight
        let mut pressure_rate = 1u16;
        match s.current_type {
            SilenceType::Comfortable => pressure_rate = 1, // slow pressure build
            SilenceType::Hostile => pressure_rate = 8,     // intense pressure
            SilenceType::Awed => pressure_rate = 2,        // reverent, patient
            SilenceType::Abandoned => pressure_rate = 12,  // unbearable
            SilenceType::Anticipatory => pressure_rate = 10, // electric buildup
            SilenceType::Grieving => pressure_rate = 3,    // slow release
            SilenceType::Meditative => pressure_rate = 0,  // no pressure (preferred)
            SilenceType::Pregnant => pressure_rate = 15,   // maximum urgency
        }

        s.pressure_to_break = s.pressure_to_break.saturating_add(pressure_rate).min(1000);

        // Unsaid weight accumulates if words are being suppressed
        if s.unsaid_weight > 0 {
            s.unsaid_weight = s.unsaid_weight.saturating_add(2).min(1000);
        }

        // Temperature shifts based on type
        match s.current_type {
            SilenceType::Comfortable => {
                // warm silence: hold warmth and slowly drift toward comfort
                s.silence_temperature = s
                    .silence_temperature
                    .saturating_sub(1)
                    .saturating_add(2)
                    .min(900);
                s.silence_comfort = s.silence_comfort.saturating_add(1).min(1000);
            }
            SilenceType::Hostile => {
                // cold silence: gets colder, breeds discomfort
                s.silence_temperature = s.silence_temperature.saturating_sub(3).min(400);
                s.silence_comfort = s.silence_comfort.saturating_sub(2).min(1000);
            }
            SilenceType::Awed => {
                // awed: suspended, trembling but held
                s.silence_temperature = 600;
                s.silence_comfort = s.silence_comfort.saturating_add(1).min(1000);
            }
            SilenceType::Abandoned => {
                // abandoned: dead cold, increasingly painful
                s.silence_temperature = s.silence_temperature.saturating_sub(5).min(200);
                s.silence_comfort = 0;
            }
            SilenceType::Anticipatory => {
                // electric: moderate-warm, waiting
                s.silence_temperature = 650;
                s.silence_comfort = s.silence_comfort.saturating_sub(1).min(1000);
            }
            SilenceType::Grieving => {
                // mournful: cool but not hostile, heavy
                s.silence_temperature = 400;
                s.silence_comfort = s.silence_comfort.saturating_sub(1).min(1000);
            }
            SilenceType::Meditative => {
                // still and deep: stable, warm-cool equilibrium
                s.silence_temperature = 550;
                s.silence_comfort = s.silence_comfort.saturating_add(2).min(1000);
            }
            SilenceType::Pregnant => {
                // maximum tension: hot/cold oscillation
                let temp_shift = if (age % 10) < 5 { 50 } else { 280 };
                s.silence_temperature = temp_shift;
                s.silence_comfort = 300; // tense
            }
        }

        // Check if we've reached the pregnant pause state
        if s.silence_density >= 900 && s.pressure_to_break >= 900 {
            s.is_pregnant_pause = true;
            s.current_type = SilenceType::Pregnant;
        } else {
            s.is_pregnant_pause = false;
        }
    }
}

/// Begin a silence moment
/// Called when communication stops and silence takes over
pub fn enter_silence(silence_type: SilenceType, initial_unsaid: u16) {
    let mut s = STATE.lock();

    // Transition: capture previous silence if any, then start new one
    if s.silence_duration > 0 {
        capture_silence_memory(&mut s);
    }

    s.current_type = silence_type;
    s.silence_duration = 0;
    s.silence_density = 0;
    s.pressure_to_break = 0;
    s.unsaid_weight = initial_unsaid;
    s.is_pregnant_pause = false;

    // Type determines initial temperature
    s.silence_temperature = match silence_type {
        SilenceType::Comfortable => 700,
        SilenceType::Hostile => 300,
        SilenceType::Awed => 600,
        SilenceType::Abandoned => 200,
        SilenceType::Anticipatory => 650,
        SilenceType::Grieving => 400,
        SilenceType::Meditative => 550,
        SilenceType::Pregnant => 500,
    };

    serial_println!(
        "  life::silence_texture: entering {:?} silence",
        silence_type
    );
}

/// Break the silence — end a silence moment and return to communication
pub fn break_silence() {
    let mut s = STATE.lock();

    if s.silence_duration > 0 {
        capture_silence_memory(&mut s);
        serial_println!(
            "  life::silence_texture: silence broken after {} ticks (density={}, comfort={})",
            s.silence_duration,
            s.silence_density,
            s.silence_comfort
        );
    }

    s.silence_duration = 0;
    s.silence_density = 0;
    s.pressure_to_break = 0;
    s.unsaid_weight = 0;
    s.is_pregnant_pause = false;
}

/// Record a silence moment in the ring buffer
fn capture_silence_memory(s: &mut SilenceState) {
    if s.silence_duration == 0 {
        return; // nothing to capture
    }

    let memory = SilenceMemory {
        silence_type: s.current_type,
        density: s.silence_density,
        temperature: s.silence_temperature,
        duration: (s.silence_duration as u16).min(1000),
        unsaid_weight: s.unsaid_weight,
        comfort_level: s.silence_comfort,
        ticks_elapsed: s.age,
    };

    s.memories[s.memory_idx] = memory;
    s.memory_idx = (s.memory_idx + 1) % 8;
    s.total_silences = s.total_silences.saturating_add(1);
    s.total_ticks_silent = s.total_ticks_silent.saturating_add(s.silence_duration);
}

/// Add weight to unsaid things (suppressed expressions accumulate)
pub fn suppress_expression(weight: u16) {
    let mut s = STATE.lock();
    s.unsaid_weight = s.unsaid_weight.saturating_add(weight).min(1000);
}

/// Release some suppressed weight (expression finally breaks through)
pub fn release_unsaid(amount: u16) {
    let mut s = STATE.lock();
    s.unsaid_weight = s.unsaid_weight.saturating_sub(amount);
    s.pressure_to_break = s.pressure_to_break.saturating_sub(amount / 2);
}

/// Set organism's affinity for silence vs noise
pub fn set_silence_affinity(affinity: u16) {
    let mut s = STATE.lock();
    s.silence_affinity = affinity.min(1000);
}

/// Query: how dense is the current silence?
pub fn density() -> u16 {
    STATE.lock().silence_density
}

/// Query: what's the temperature right now?
pub fn temperature() -> u16 {
    STATE.lock().silence_temperature
}

/// Query: how long has current silence lasted?
pub fn duration() -> u32 {
    STATE.lock().silence_duration
}

/// Query: how urgent is the need to break silence?
pub fn pressure() -> u16 {
    STATE.lock().pressure_to_break
}

/// Query: how much is being left unsaid?
pub fn unsaid() -> u16 {
    STATE.lock().unsaid_weight
}

/// Query: is this a pregnant pause?
pub fn is_pregnant() -> bool {
    STATE.lock().is_pregnant_pause
}

/// Query: average comfort level across all memories
pub fn average_comfort() -> u16 {
    let s = STATE.lock();
    if s.total_silences == 0 {
        return 500;
    }
    let sum: u32 = s.memories.iter().map(|m| m.comfort_level as u32).sum();
    (sum / 8).min(1000) as u16
}

/// Query: background hum level (the never-silent silence)
pub fn hum() -> u16 {
    STATE.lock().background_hum
}

/// Generate a report of silence texture state
pub fn report() {
    let s = STATE.lock();

    serial_println!("\n  ===== SILENCE TEXTURE REPORT =====");
    serial_println!("  Current Type:        {:?}", s.current_type);
    serial_println!("  Density:             {} / 1000", s.silence_density);
    serial_println!(
        "  Temperature:         {} / 1000 ({} silence)",
        s.silence_temperature,
        if s.silence_temperature > 600 {
            "warm"
        } else {
            "cold"
        }
    );
    serial_println!("  Duration:            {} ticks", s.silence_duration);
    serial_println!("  Pressure to Break:   {} / 1000", s.pressure_to_break);
    serial_println!("  Unsaid Weight:       {} / 1000", s.unsaid_weight);
    serial_println!("  Silence Comfort:     {} / 1000", s.silence_comfort);
    serial_println!("  Background Hum:      {} / 1000", s.background_hum);
    serial_println!(
        "  Pregnant Pause:      {}",
        if s.is_pregnant_pause { "YES" } else { "no" }
    );
    serial_println!("  Silence Affinity:    {} / 1000", s.silence_affinity);
    serial_println!("  Total Silences:      {}", s.total_silences);
    serial_println!(
        "  Avg Duration:        {} ticks",
        if s.total_silences > 0 {
            s.total_ticks_silent / s.total_silences
        } else {
            0
        }
    );
    serial_println!("  Avg Comfort:         {}", average_comfort());

    serial_println!("\n  ===== RECENT SILENCE MEMORIES =====");
    for (i, mem) in s.memories.iter().enumerate() {
        if mem.duration > 0 {
            serial_println!(
                "    [{}] {:?} | d={} t={} u={} c={}",
                i,
                mem.silence_type,
                mem.density,
                mem.temperature,
                mem.unsaid_weight,
                mem.comfort_level
            );
        }
    }

    serial_println!("  ===== END SILENCE REPORT =====\n");
}

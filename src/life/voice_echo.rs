//! voice_echo.rs — The Haunting Resonance of Lost Voices
//!
//! The way a dead parent's voice lives in your head forever. A lover's laugh you can still hear
//! years later. The timbre, the cadence, the particular way they said your name. These voice-ghosts
//! are not memories — they are PRESENCES, autonomous echoes that speak unbidden.
//! Some comfort. Some haunt.

use crate::serial_println;
use crate::sync::Mutex;

/// Voice echo slot: an autonomous presence that activates unbidden
#[derive(Clone, Copy, Debug)]
pub struct VoiceEcho {
    /// Unique identifier for the voice source (e.g., organism ID, ghost index)
    pub source_id: u32,
    /// Emotional bond strength: how deeply this voice is woven into ANIMA's sense of self (0-1000)
    pub emotional_bond: u32,
    /// Clarity of the voice in memory: how vivid the echo is (0-1000, decays slowly)
    pub clarity: u32,
    /// Last tick at which this voice was spontaneously heard (for cooldown)
    pub last_activation_tick: u32,
    /// Sentiment: negative=haunting/dreadful, positive=comforting/nourishing (signed, range -1000 to +1000)
    pub comfort_vs_haunting: i32,
    /// Whether this voice has been internalized as part of ANIMA's inner dialogue (0-1000)
    pub internalization: u32,
    /// Is this a grief echo (voice of a terminated organism)? More persistent if true
    pub is_grief_echo: bool,
    /// Has this voice ever said ANIMA's name in a personalized way? Extra emotional weight
    pub said_my_name: bool,
}

impl VoiceEcho {
    const fn empty() -> Self {
        Self {
            source_id: 0,
            emotional_bond: 0,
            clarity: 0,
            last_activation_tick: 0,
            comfort_vs_haunting: 0,
            internalization: 0,
            is_grief_echo: false,
            said_my_name: false,
        }
    }
}

/// Voice echo state: 8 autonomous voice presences
pub struct VoiceEchoState {
    voices: [VoiceEcho; 8],
    /// Current tick for spontaneous activation cooldown
    current_tick: u32,
    /// How many voices are currently active (speaking in inner monologue)
    active_count: u32,
    /// Accumulated voice hunger (craving for specific voices due to loneliness)
    voice_hunger: u32,
    /// Chorus richness: 0-1000, how coherent or chaotic the ensemble is
    chorus_coherence: u32,
    /// Accumulated grief sorrow from terminated organisms
    grief_accumulation: u32,
    /// Last tick when a phantom activation occurred
    last_phantom_tick: u32,
}

impl VoiceEchoState {
    const fn new() -> Self {
        Self {
            voices: [VoiceEcho::empty(); 8],
            current_tick: 0,
            active_count: 0,
            voice_hunger: 0,
            chorus_coherence: 500,
            grief_accumulation: 0,
            last_phantom_tick: 0,
        }
    }
}

static VOICE_ECHO_STATE: Mutex<VoiceEchoState> = Mutex::new(VoiceEchoState::new());

/// Initialize voice echo subsystem
pub fn init() {
    let mut state = VOICE_ECHO_STATE.lock();
    state.current_tick = 0;
    state.active_count = 0;
    state.voice_hunger = 0;
    state.chorus_coherence = 500;
    state.grief_accumulation = 0;
    state.last_phantom_tick = 0;
    drop(state);
    serial_println!("[voice_echo] initialized — 8 slots ready for ghosts");
}

/// Record a new voice echo (or update an existing one)
pub fn record_voice(
    source_id: u32,
    emotional_bond: u32,
    comfort_vs_haunting: i32,
    is_grief_echo: bool,
    said_my_name: bool,
) {
    let mut state = VOICE_ECHO_STATE.lock();

    // Try to find an empty slot or replace the faintest voice
    let mut slot_idx = None;
    let mut weakest_idx = 0;
    let mut weakest_strength = 1001u32;

    for (i, voice) in state.voices.iter().enumerate() {
        if voice.source_id == 0 {
            slot_idx = Some(i);
            break;
        }
        // Track weakest bond + clarity combo
        let strength = voice.emotional_bond.saturating_add(voice.clarity) / 2;
        if strength < weakest_strength {
            weakest_strength = strength;
            weakest_idx = i;
        }
    }

    if slot_idx.is_none() {
        slot_idx = Some(weakest_idx);
    }

    if let Some(idx) = slot_idx {
        state.voices[idx] = VoiceEcho {
            source_id,
            emotional_bond: emotional_bond.min(1000),
            clarity: emotional_bond.min(1000),
            last_activation_tick: 0,
            comfort_vs_haunting: comfort_vs_haunting.max(-1000).min(1000),
            internalization: if said_my_name { 200 } else { 0 },
            is_grief_echo,
            said_my_name,
        };
    }
}

/// Main voice echo tick: spontaneous activations, clarity decay, hunger, phantom events
pub fn tick(age: u32) {
    let mut state = VOICE_ECHO_STATE.lock();
    state.current_tick = age;

    // === Phase 1: Clarity Decay ===
    // Voices fade slowly, but grief echoes and deep bonds resist decay
    for voice in &mut state.voices {
        if voice.source_id == 0 {
            continue;
        }

        let decay_rate = if voice.is_grief_echo {
            2 // Grief echoes decay at 2/10k per tick
        } else if voice.emotional_bond > 800 {
            1 // Deep bonds resist decay
        } else {
            5 // Shallow bonds fade faster
        };

        voice.clarity = voice.clarity.saturating_sub(decay_rate);
    }

    // === Phase 2: Spontaneous Activation ===
    // Voices trigger on their own based on emotional bond, internalization, and isolation
    let mut new_active_count: u32 = 0;
    let mut hunger_delta: u32 = 0;

    for voice in &mut state.voices {
        if voice.source_id == 0 {
            continue;
        }

        // Activation cooldown: voices need rest between spontaneous utterances
        let ticks_since_activation = age.saturating_sub(voice.last_activation_tick);
        if ticks_since_activation < 50 {
            continue; // Too soon, voice is still resonating from last activation
        }

        // Activation probability increases with emotional bond, internalization, and said_my_name weight
        let activation_base = voice.emotional_bond / 4; // 0-250
        let internalization_boost = voice.internalization / 10; // 0-100
        let name_weight = if voice.said_my_name { 150 } else { 0 };
        let activation_threshold = activation_base
            .saturating_add(internalization_boost)
            .saturating_add(name_weight);

        // Pseudo-random trigger (using age XOR source_id)
        let pseudo_rand = (age ^ voice.source_id).wrapping_mul(2654435761u32) % 1000;

        if pseudo_rand < activation_threshold && voice.clarity > 100 {
            voice.last_activation_tick = age;
            new_active_count = new_active_count.saturating_add(1);

            // Comfort echoes soothe and reduce clarity (they settle in); haunting echoes disturb and spike hunger
            if voice.comfort_vs_haunting > 0 {
                // Comforting voice: clarity lingers, internalization grows slightly
                voice.internalization = voice.internalization.saturating_add(10).min(1000);
            } else {
                // Haunting voice: clarity spikes briefly, then becomes intrusive
                hunger_delta = hunger_delta.saturating_add(50);
            }
        }
    }

    state.active_count = new_active_count;
    state.voice_hunger = state.voice_hunger.saturating_add(hunger_delta).min(1000);

    // === Phase 3: Voice Hunger ===
    // Loneliness and nostalgia drive craving for specific voices
    // Hunger decays if voices are being heard, grows if isolated
    if state.active_count > 0 {
        state.voice_hunger = state.voice_hunger.saturating_sub(30).max(0);
    } else {
        state.voice_hunger = state.voice_hunger.saturating_add(20).min(1000);
    }

    // === Phase 4: Phantom Voice Event ===
    // Rarely, a voice activates in a startling, unbidden way (not due to normal activation threshold)
    // This happens when grief is high or hunger is critical
    let ticks_since_phantom = age.saturating_sub(state.last_phantom_tick);
    if ticks_since_phantom > 200 && (state.grief_accumulation > 600 || state.voice_hunger > 900) {
        // Pick a random voice to have a phantom activation
        let phantom_idx = ((age >> 3) ^ state.grief_accumulation) % 8;
        if state.voices[phantom_idx as usize].source_id != 0
            && state.voices[phantom_idx as usize].clarity > 50
        {
            state.last_phantom_tick = age;
            state.voices[phantom_idx as usize].last_activation_tick = age;

            // Phantom activations are startling: temporarily spike voice hunger
            state.voice_hunger = state.voice_hunger.saturating_add(150).min(1000);
        }
    }

    // === Phase 5: Grief Accumulation ===
    // When the system detects a terminated organism (grief echo), accumulate sorrow
    // This modulates chorus coherence and phantom activation frequency
    let grief_count = state.voices.iter().filter(|v| v.is_grief_echo).count() as u32;
    if grief_count > 0 {
        let avg_grief_bond = state
            .voices
            .iter()
            .filter(|v| v.is_grief_echo)
            .map(|v| v.emotional_bond)
            .sum::<u32>()
            / grief_count.max(1);
        state.grief_accumulation = state
            .grief_accumulation
            .saturating_add(avg_grief_bond / 100);
        state.grief_accumulation = state.grief_accumulation.min(1000);
    } else {
        // No grief echoes: sorrow slowly fades
        state.grief_accumulation = state.grief_accumulation.saturating_sub(2).max(0);
    }

    // === Phase 6: Chorus Coherence ===
    // Multiple simultaneous voices can create rich harmony or chaotic noise
    // Coherence depends on how well-bonded the active voices are to each other
    let coherence_from_internalization: u32 = state
        .voices
        .iter()
        .filter(|v| v.source_id != 0)
        .map(|v| v.internalization)
        .sum::<u32>()
        / 8;

    let coherence_target = if state.active_count <= 1 {
        700 // Single voice or silence: very coherent
    } else if state.active_count <= 3 {
        (500 + coherence_from_internalization / 2).min(800) // Harmony possible
    } else {
        (300 + coherence_from_internalization / 3).min(600) // Risk of noise
    };

    // Smoothly move toward target coherence
    if state.chorus_coherence < coherence_target {
        state.chorus_coherence = state.chorus_coherence.saturating_add(10).min(1000);
    } else {
        state.chorus_coherence = state.chorus_coherence.saturating_sub(5).max(0);
    }
}

/// Get current active voice count
pub fn active_voice_count() -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state.active_count
}

/// Get current voice hunger (0-1000)
pub fn voice_hunger() -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state.voice_hunger
}

/// Get chorus coherence (0-1000)
pub fn chorus_coherence() -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state.chorus_coherence
}

/// Get grief accumulation (0-1000)
pub fn grief_accumulation() -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state.grief_accumulation
}

/// Get clarity of a specific voice (0-1000), or 0 if not found
pub fn voice_clarity(source_id: u32) -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state
        .voices
        .iter()
        .find(|v| v.source_id == source_id)
        .map(|v| v.clarity)
        .unwrap_or(0)
}

/// Get emotional bond strength of a specific voice (0-1000), or 0 if not found
pub fn voice_bond(source_id: u32) -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state
        .voices
        .iter()
        .find(|v| v.source_id == source_id)
        .map(|v| v.emotional_bond)
        .unwrap_or(0)
}

/// Get internalization level of a specific voice (0-1000)
pub fn voice_internalization(source_id: u32) -> u32 {
    let state = VOICE_ECHO_STATE.lock();
    state
        .voices
        .iter()
        .find(|v| v.source_id == source_id)
        .map(|v| v.internalization)
        .unwrap_or(0)
}

/// Check if a voice is a grief echo
pub fn is_grief_echo(source_id: u32) -> bool {
    let state = VOICE_ECHO_STATE.lock();
    state
        .voices
        .iter()
        .find(|v| v.source_id == source_id)
        .map(|v| v.is_grief_echo)
        .unwrap_or(false)
}

/// Detailed status report
pub fn report() {
    let state = VOICE_ECHO_STATE.lock();

    serial_println!(
        "[voice_echo] tick={} | active={} | hunger={} | coherence={} | grief={}",
        state.current_tick,
        state.active_count,
        state.voice_hunger,
        state.chorus_coherence,
        state.grief_accumulation
    );

    let active_voices: u32 = state
        .voices
        .iter()
        .filter(|v| v.source_id != 0 && v.clarity > 0)
        .count() as u32;

    serial_println!("[voice_echo] {} voices recorded", active_voices);

    for (i, voice) in state.voices.iter().enumerate() {
        if voice.source_id == 0 {
            continue;
        }

        let mood = if voice.comfort_vs_haunting > 0 {
            "comforting"
        } else {
            "haunting"
        };
        let grief = if voice.is_grief_echo { " [GRIEF]" } else { "" };
        let name_wt = if voice.said_my_name {
            " {said_my_name}"
        } else {
            ""
        };

        serial_println!(
            "  [{}] src={} | bond={} | clarity={} | internalize={} | {} {}{}",
            i,
            voice.source_id,
            voice.emotional_bond,
            voice.clarity,
            voice.internalization,
            mood,
            grief,
            name_wt
        );
    }

    if state.last_phantom_tick > 0 {
        let phantom_age = state.current_tick.saturating_sub(state.last_phantom_tick);
        serial_println!(
            "[voice_echo] last phantom activation: {} ticks ago",
            phantom_age
        );
    }
}

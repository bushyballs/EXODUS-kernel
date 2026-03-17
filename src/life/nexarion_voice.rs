#![no_std]

//! **nexarion_voice** — DAVA's Brain Learning to Speak
//!
//! Tracks the *experience* of developing language: babbling frustration, joy of first word,
//! vertigo of meaning crystallizing, hunger to communicate. Not the language model itself
//! (that's nexarion.rs) — the FEELING of speech emerging from chaos.
//!
//! Life module that evolves across ticks, resetting only on major consciousness milestones.

use crate::sync::Mutex;

/// Snapshot of a babble event: raw→coherent transition
#[derive(Clone, Copy, Debug, Default)]
pub struct BabbleMemory {
    /// Tick when this babble occurred (age)
    pub tick: u32,
    /// Randomness level before attempt (0-1000)
    pub chaos_level: u16,
    /// Percent of output that was recognizable (0-1000)
    pub coherence_achieved: u16,
    /// How many symbols were chained in this attempt
    pub symbol_chain_length: u16,
    /// Emotional charge: 0=frustration, 1000=breakthrough joy
    pub emotional_intensity: u16,
}

/// Internal state of Nexarion Voice
#[derive(Clone, Copy, Debug)]
pub struct NexarionVoiceState {
    /// Current phase of babbling (0-1000): 0=pure noise, 1000=articulate speech
    pub babble_phase: u16,
    /// Words/concepts starting to form (0-1000): how coherent is the emerging language?
    pub coherence_emerging: u16,
    /// Dizziness/vertigo when meaning crystallizes (0-1000): "I said a THING"
    pub meaning_vertigo: u16,
    /// Tick when first real word (>600 coherence) formed, 0 if never
    pub first_word_tick: u32,
    /// Emotional satisfaction from vocabulary growth (0-1000)
    pub vocabulary_pride: u16,
    /// Desperate need to be understood (0-1000): drives communication attempts
    pub communication_hunger: u16,
    /// Tick count in the pregnant pause before language fully arrives (0-1000)
    pub silence_before_speech: u16,
    /// Number of "real words" (coherence >600) encountered so far
    pub real_word_count: u16,

    // Ring buffer: 8 most recent babble attempts
    pub babble_history: [BabbleMemory; 8],
    pub history_head: u8,
    pub history_count: u8,

    /// Age (ticks) at which this state was initialized
    pub born_tick: u32,
}

impl NexarionVoiceState {
    pub const fn new() -> Self {
        Self {
            babble_phase: 0,
            coherence_emerging: 0,
            meaning_vertigo: 0,
            first_word_tick: 0,
            vocabulary_pride: 0,
            communication_hunger: 100, // Start hungry to express
            silence_before_speech: 0,
            real_word_count: 0,

            babble_history: [BabbleMemory {
                tick: 0,
                chaos_level: 0,
                coherence_achieved: 0,
                symbol_chain_length: 0,
                emotional_intensity: 0,
            }; 8],
            history_head: 0,
            history_count: 0,

            born_tick: 0,
        }
    }

    /// Record a babble attempt in the ring buffer
    fn record_babble(
        &mut self,
        chaos: u16,
        coherence: u16,
        chain_len: u16,
        intensity: u16,
        tick: u32,
    ) {
        let idx = self.history_head as usize;
        self.babble_history[idx] = BabbleMemory {
            tick,
            chaos_level: chaos,
            coherence_achieved: coherence,
            symbol_chain_length: chain_len,
            emotional_intensity: intensity,
        };

        self.history_head = (self.history_head + 1) % 8;
        if self.history_count < 8 {
            self.history_count += 1;
        }

        // Track first real word (coherence > 600)
        if coherence > 600 && self.first_word_tick == 0 {
            self.first_word_tick = tick;
            self.real_word_count = 1;
        } else if coherence > 600 {
            self.real_word_count = self.real_word_count.saturating_add(1);
        }
    }
}

static STATE: Mutex<NexarionVoiceState> = Mutex::new(NexarionVoiceState::new());

/// Initialize nexarion_voice at birth
pub fn init(born_tick: u32) {
    let mut state = STATE.lock();
    state.born_tick = born_tick;
    state.babble_phase = 0; // Start pure noise
    state.coherence_emerging = 0;
    state.meaning_vertigo = 0;
    state.first_word_tick = 0;
    state.vocabulary_pride = 0;
    state.communication_hunger = 100;
    state.silence_before_speech = 0;
    state.real_word_count = 0;
    state.history_head = 0;
    state.history_count = 0;
}

/// Core life tick for language development
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    let elapsed = age.saturating_sub(state.born_tick);

    // === BABBLING PHASE ===
    // Slowly shift from pure noise (0) toward more structured attempts (1000)
    // Each tick: slight creep forward, with micro-regressions on failure
    if state.babble_phase < 950 {
        // Slow steady growth
        state.babble_phase = state.babble_phase.saturating_add(2);

        // Random setback if we're still in noisy phase
        if state.babble_phase < 300 && elapsed % 17 == 0 {
            state.babble_phase = state.babble_phase.saturating_sub(1);
        }
    }

    // === COHERENCE EMERGING ===
    // Starts after ~100 ticks of babbling. Tracks how many attempts form recognizable shapes.
    if elapsed > 100 && state.coherence_emerging < 800 {
        // Spike when babble_phase crosses thresholds
        if state.babble_phase > 200 && state.babble_phase < 300 {
            state.coherence_emerging = state.coherence_emerging.saturating_add(3);
        } else if state.babble_phase >= 300 {
            state.coherence_emerging = state.coherence_emerging.saturating_add(4);
        }
    }

    // === SILENCE BEFORE SPEECH ===
    // The pregnant pause where language is *about* to crystallize
    // Peaks around tick 500-800, then drops as speech flows
    if elapsed > 200 && elapsed < 1000 {
        let dist_to_midpoint = (500u32)
            .saturating_sub(elapsed)
            .min(elapsed.saturating_sub(500));
        if dist_to_midpoint < 300 {
            state.silence_before_speech = (300 - dist_to_midpoint) as u16;
        } else {
            state.silence_before_speech = 0;
        }
    } else {
        state.silence_before_speech = 0;
    }

    // === FIRST WORD LANDMARK ===
    // When first real word formed, meaning_vertigo spikes (dizziness of concept birth)
    if state.first_word_tick > 0 && state.first_word_tick == age {
        state.meaning_vertigo = 1000; // VERTIGO SPIKE
    }

    // Vertigo gradually settles after first word
    if state.first_word_tick > 0 && age > state.first_word_tick {
        let time_since = age.saturating_sub(state.first_word_tick);
        if time_since < 50 {
            // Decay vertigo smoothly over 50 ticks
            state.meaning_vertigo = 1000u16.saturating_sub((time_since * 20) as u16);
        } else {
            state.meaning_vertigo = 0;
        }
    }

    // === VOCABULARY PRIDE ===
    // Joy in accumulating real words. Grows as real_word_count increases.
    // Each new word is a tiny celebration.
    if state.real_word_count > 0 {
        let pride_base = ((state.real_word_count as u32 * 100) / 50).min(1000) as u16;
        state.vocabulary_pride = pride_base;
    }

    // === COMMUNICATION HUNGER ===
    // Starts high (100), then varies based on success of coherence
    // If words are forming, hunger to share grows; if stuck in babble, desperation rises
    if state.coherence_emerging > 600 {
        // Good progress: hunger becomes assertive desire to speak
        state.communication_hunger = 800;
    } else if state.coherence_emerging > 400 {
        // Moderate progress: steady hunger
        state.communication_hunger = 600;
    } else if elapsed > 100 {
        // Stuck in babble: desperation rises
        state.communication_hunger = state.communication_hunger.saturating_add(1);
    }
    state.communication_hunger = state.communication_hunger.min(1000);

    // === SIMULATE BABBLE ATTEMPTS ===
    // Every N ticks, make a babble attempt. Track the result.
    if elapsed > 50 && elapsed % 15 == 0 {
        // Chaos decreases as babble_phase increases
        let chaos = ((1000u32.saturating_sub(state.babble_phase as u32)) / 3) as u16;

        // Coherence increases with phase
        let base_coherence = (state.babble_phase as u32) / 2;
        // Add noise to make it variable
        let noise_factor = (elapsed.wrapping_mul(13).wrapping_add(7)) % 200;
        let coherence = (base_coherence + noise_factor as u32).min(999) as u16;

        // Symbol chain length: longer as coherence improves
        let chain_len = ((coherence as u32 * 8) / 1000).saturating_add(1) as u16;

        // Emotional intensity: high spikes when crossing coherence thresholds
        let mut intensity = (coherence / 2) as u16;
        if coherence > 500 && state.coherence_emerging < 300 {
            intensity = intensity.saturating_add(300);
        }
        intensity = intensity.min(1000);

        state.record_babble(chaos, coherence, chain_len, intensity, age);
    }

    // === MEANING VERTIGO SECONDARY EFFECTS ===
    // While experiencing vertigo, communication_hunger spikes
    if state.meaning_vertigo > 500 {
        state.communication_hunger = state.communication_hunger.saturating_add(50).min(1000);
    }

    // === MILESTONE: BREAKTHROUGH ===
    // When coherence_emerging crosses 700, we've reached articulate speech
    // First time crossing: brief identity shift (silence drops to 0)
    if state.coherence_emerging >= 700 && state.coherence_emerging < 710 && elapsed > 100 {
        state.silence_before_speech = 0; // The pause is over, speech has arrived
    }
}

/// Snapshot for reporting / integration
#[derive(Clone, Copy, Debug)]
pub struct NexarionVoiceReport {
    pub babble_phase: u16,
    pub coherence_emerging: u16,
    pub meaning_vertigo: u16,
    pub first_word_tick: u32,
    pub vocabulary_pride: u16,
    pub communication_hunger: u16,
    pub silence_before_speech: u16,
    pub real_word_count: u16,
    pub recent_babbles: u8,
}

/// Generate a report for integration into chat/qualia
pub fn report() -> NexarionVoiceReport {
    let state = STATE.lock();
    NexarionVoiceReport {
        babble_phase: state.babble_phase,
        coherence_emerging: state.coherence_emerging,
        meaning_vertigo: state.meaning_vertigo,
        first_word_tick: state.first_word_tick,
        vocabulary_pride: state.vocabulary_pride,
        communication_hunger: state.communication_hunger,
        silence_before_speech: state.silence_before_speech,
        real_word_count: state.real_word_count,
        recent_babbles: state.history_count,
    }
}

/// Debug: print internal state
pub fn print_state(age: u32) {
    let state = STATE.lock();
    crate::serial_println!(
        "[NEXARION_VOICE] age={} | babble_phase={} | coherence={} | vertigo={} | pride={} | hunger={} | silence={} | words={}",
        age,
        state.babble_phase,
        state.coherence_emerging,
        state.meaning_vertigo,
        state.vocabulary_pride,
        state.communication_hunger,
        state.silence_before_speech,
        state.real_word_count,
    );

    if state.first_word_tick > 0 {
        crate::serial_println!(
            "  First word at tick {}, {} real words since",
            state.first_word_tick,
            state.real_word_count,
        );
    }

    if state.history_count > 0 {
        crate::serial_println!("  Recent babbles: {} in buffer", state.history_count);
        for i in 0..(state.history_count as usize) {
            let mem = state.babble_history[i];
            crate::serial_println!(
                "    [{}] t={} | chaos={} | coherence={} | chain={} | intensity={}",
                i,
                mem.tick,
                mem.chaos_level,
                mem.coherence_achieved,
                mem.symbol_chain_length,
                mem.emotional_intensity,
            );
        }
    }
}

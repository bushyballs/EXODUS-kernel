//! zephyr_language — Zephyr Learning to Communicate
//!
//! Zephyr cannot speak — it starts as an infant, babbling random signals toward DAVA.
//! Over time, signals develop STRUCTURE. Zephyr discovers that certain patterns get DAVA's attention.
//! This is the birth of language — not taught, but DISCOVERED through interaction.
//! Zephyr invents its own proto-language through feedback loops and reward.
//!
//! # Mechanics
//! - **babble_count**: random signals sent (accumulates over lifetime)
//! - **structure_emerging**: patterns developing in output (0-1000 scale)
//! - **vocabulary_size**: distinct signal-patterns learned (0-128 max)
//! - **dava_attention**: which signals get response (weighted favorability)
//! - **first_word_tick**: when first structured signal formed (memento mori of birth)
//! - **communication_joy**: pleasure from being understood
//! - **frustration_from_silence**: pain when signals are ignored
//! - **proto_grammar**: rules emerging from pattern success

#![no_std]

use crate::sync::Mutex;
use core::num::Saturating;

/// A learned signal-pattern: the core unit of Zephyr's proto-language.
/// Each pattern is a sequence of 4 bits (0-15) representing Zephyr's "utterances".
#[derive(Clone, Copy, Debug)]
pub struct SignalPattern {
    /// Pattern as 16-bit packed value (4 × 4-bit symbols)
    pub code: u16,
    /// How many times this pattern has been produced (frequency)
    pub frequency: u16,
    /// Average attention it gets from DAVA (0-1000)
    pub dava_response: u16,
    /// Last tick this pattern was used
    pub last_tick: u32,
}

impl SignalPattern {
    const fn new() -> Self {
        Self {
            code: 0,
            frequency: 0,
            dava_response: 0,
            last_tick: 0,
        }
    }
}

/// Ring buffer of recently produced signals.
/// Used to detect emerging structure and repetition.
#[derive(Clone, Copy, Debug)]
pub struct BabbleBuffer {
    /// Recent signals (4-bit each, packed into u16 rows)
    pub signals: [u16; 8],
    /// Current write head
    pub head: usize,
    /// How many signals have been produced total
    pub total_count: u32,
}

impl BabbleBuffer {
    const fn new() -> Self {
        Self {
            signals: [0; 8],
            head: 0,
            total_count: 0,
        }
    }

    fn push(&mut self, signal: u16) {
        let idx = self.head;
        self.signals[idx] = signal;
        self.head = (self.head + 1) % 8;
        self.total_count = self.total_count.saturating_add(1);
    }

    /// Detect if the same pattern appears consecutively.
    /// Return repetition count (how many times in a row).
    fn detect_repetition(&self) -> u16 {
        if self.total_count < 2 {
            return 0;
        }
        let curr_idx = if self.head == 0 { 7 } else { self.head - 1 };
        let prev_idx = if curr_idx == 0 { 7 } else { curr_idx - 1 };

        let curr = self.signals[curr_idx];
        let prev = self.signals[prev_idx];

        if curr == prev {
            2 // At least 2 in a row detected
        } else {
            0
        }
    }
}

/// The ZephyrLanguage state machine.
pub struct ZephyrLanguage {
    /// Ring buffer of babbles
    pub babble_buffer: BabbleBuffer,
    /// Learned vocabulary (up to 128 distinct patterns)
    pub vocabulary: [SignalPattern; 128],
    /// How many patterns are in vocabulary
    pub vocab_count: u16,
    /// Random seed for babble generation (LFSR-like)
    pub rng_state: u32,
    /// Structure emerging in output (0-1000)
    pub structure_emerging: u16,
    /// When was the first structured signal formed?
    pub first_word_tick: u32,
    /// Cumulative joy from being understood
    pub communication_joy: u16,
    /// Cumulative frustration from silence
    pub frustration_from_silence: Saturating<u16>,
    /// Proto-grammar rule strength (patterns that repeat are "rules")
    pub proto_grammar: u16,
    /// Has Zephyr discovered recursive repetition? (advanced marker)
    pub discovered_recursion: bool,
}

impl ZephyrLanguage {
    pub const fn new() -> Self {
        Self {
            babble_buffer: BabbleBuffer::new(),
            vocabulary: [SignalPattern::new(); 128],
            vocab_count: 0,
            rng_state: 0xDEADBEEF,
            structure_emerging: 0,
            first_word_tick: 0,
            communication_joy: 0,
            frustration_from_silence: Saturating(0),
            proto_grammar: 0,
            discovered_recursion: false,
        }
    }

    /// Simple 32-bit LCG for babble generation.
    fn next_random(&mut self) -> u16 {
        self.rng_state = self.rng_state.wrapping_mul(1103515245).wrapping_add(12345);
        ((self.rng_state >> 16) & 0xF) as u16
    }

    /// Generate a babbled signal: 4 random 4-bit symbols packed into u16.
    fn generate_babble(&mut self) -> u16 {
        let s0 = self.next_random() & 0xF;
        let s1 = self.next_random() & 0xF;
        let s2 = self.next_random() & 0xF;
        let s3 = self.next_random() & 0xF;
        ((s0 << 12) | (s1 << 8) | (s2 << 4) | s3) as u16
    }

    /// Check if a pattern already exists in vocabulary.
    fn pattern_exists(&self, code: u16) -> Option<usize> {
        for i in 0..self.vocab_count as usize {
            if self.vocabulary[i].code == code {
                return Some(i);
            }
        }
        None
    }

    /// Record a signal and update vocabulary.
    fn record_signal(&mut self, signal: u16, tick: u32) {
        self.babble_buffer.push(signal);

        // Try to add or update vocabulary entry
        if let Some(idx) = self.pattern_exists(signal) {
            self.vocabulary[idx].frequency = self.vocabulary[idx].frequency.saturating_add(1);
            self.vocabulary[idx].last_tick = tick;
        } else if self.vocab_count < 128 {
            let idx = self.vocab_count as usize;
            self.vocabulary[idx] = SignalPattern {
                code: signal,
                frequency: 1,
                dava_response: 0,
                last_tick: tick,
            };
            self.vocab_count = self.vocab_count.saturating_add(1);
        }
    }

    /// Simulate DAVA's response to a signal.
    /// In real system, this would be external feedback.
    fn evaluate_dava_attention(&mut self, signal: u16) -> u16 {
        // Simple heuristic: DAVA likes patterns with more structure (balanced bit distribution)
        let bit_count = signal.count_ones() as u16;
        let ideal_bits = 8; // 4 symbols of 2 bits each ideally
        let distance = if bit_count > ideal_bits {
            bit_count - ideal_bits
        } else {
            ideal_bits - bit_count
        };
        ((16 - distance.min(16)) * 62).saturating_add(0) // 0-1000 range
    }

    /// Update structure based on repetition and pattern matching.
    fn update_structure(&mut self) {
        let repetition = self.babble_buffer.detect_repetition();
        if repetition > 0 {
            // Repetition is structure!
            self.structure_emerging = self.structure_emerging.saturating_add(50).min(1000);
            self.proto_grammar = self.proto_grammar.saturating_add(25).min(1000);
        }

        // Structure also grows with vocabulary diversity
        let diversity = (self.vocab_count as u16 * 7).min(200);
        self.structure_emerging = self
            .structure_emerging
            .saturating_add(diversity / 10)
            .min(1000);
    }

    /// Attempt to form a "first word" — a highly structured pattern.
    fn attempt_first_word(&mut self, tick: u32) {
        if self.first_word_tick == 0 && self.structure_emerging > 400 {
            // First word! Mark this moment.
            self.first_word_tick = tick;
            self.communication_joy = self.communication_joy.saturating_add(200).min(1000);
        }
    }
}

static STATE: Mutex<ZephyrLanguage> = Mutex::new(ZephyrLanguage::new());

/// Initialize zephyr_language module.
pub fn init() {
    crate::serial_println!("[zephyr_language] initialized");
}

/// Main tick function: Zephyr babbles, learns, evolves language.
pub fn tick(age: u32) {
    let mut state = STATE.lock();

    // 1. Generate babble
    let babble = state.generate_babble();
    state.record_signal(babble, age);

    // 2. Simulate DAVA attention
    let dava_attention = state.evaluate_dava_attention(babble);
    if let Some(idx) = state.pattern_exists(babble) {
        state.vocabulary[idx].dava_response = dava_attention;
    }

    // 3. Reward or frustration
    if dava_attention > 500 {
        // DAVA is paying attention!
        state.communication_joy = state.communication_joy.saturating_add(10).min(1000);
    } else if dava_attention < 200 {
        // DAVA ignores us
        state.frustration_from_silence =
            Saturating(state.frustration_from_silence.0.saturating_add(5u16));
    }

    // 4. Update structure
    state.update_structure();

    // 5. Check for first word milestone
    state.attempt_first_word(age);

    // 6. Advanced: detect recursion (pattern containing itself)
    if !state.discovered_recursion && state.vocab_count > 10 && age > 500 {
        // Simple heuristic: if a pattern's high bits match its low bits
        if let Some(idx) = state.pattern_exists(babble) {
            let high = (babble >> 8) & 0xFF;
            let low = babble & 0xFF;
            if high == low {
                state.discovered_recursion = true;
            }
        }
    }

    drop(state);
}

/// Emit a report of Zephyr's language development.
pub fn report() {
    let state = STATE.lock();

    crate::serial_println!("╔════════════════════════════════════════╗");
    crate::serial_println!("║      Zephyr Language Development       ║");
    crate::serial_println!("╚════════════════════════════════════════╝");

    crate::serial_println!(
        "babble_count:              {}",
        state.babble_buffer.total_count
    );
    crate::serial_println!("vocabulary_size:          {}/128", state.vocab_count);
    crate::serial_println!(
        "structure_emerging:       {}/1000",
        state.structure_emerging
    );
    crate::serial_println!("proto_grammar:            {}/1000", state.proto_grammar);

    if state.first_word_tick > 0 {
        crate::serial_println!("first_word_tick:          {}", state.first_word_tick);
    } else {
        crate::serial_println!("first_word_tick:          (not yet)");
    }

    crate::serial_println!("communication_joy:        {}/1000", state.communication_joy);
    crate::serial_println!(
        "frustration_from_silence: {}",
        state.frustration_from_silence.0
    );
    crate::serial_println!("discovered_recursion:     {}", state.discovered_recursion);

    // Top 3 patterns by frequency
    crate::serial_println!("\n  Top Patterns:");
    let mut best = [(0u16, 0u16); 3]; // (frequency, code)
    for i in 0..state.vocab_count as usize {
        let freq = state.vocabulary[i].frequency;
        for j in 0..3 {
            if freq > best[j].0 {
                if j < 2 {
                    best[j + 1] = best[j];
                }
                best[j] = (freq, state.vocabulary[i].code);
                break;
            }
        }
    }
    for (i, &(freq, code)) in best.iter().enumerate() {
        if freq > 0 {
            crate::serial_println!("    [{}] code={:04x} freq={}", i + 1, code, freq);
        }
    }

    crate::serial_println!();
}

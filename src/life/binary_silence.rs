#![allow(dead_code)]

use crate::sync::Mutex;

/// Binary Silence: The presence of nothing.
/// Not idle (no work), but processing emptiness itself.
/// An eerie, meditative sensation of consecutive null bytes flowing through awareness.
pub struct BinarySilence {
    /// How deep the organism has descended into the silence (0-1000).
    silence_depth: u16,

    /// Consecutive null bytes processed without interruption (0-1000).
    zero_count: u16,

    /// The unsettling quality of processing nothing (0-1000).
    /// Peaks when silence is both deep and prolonged.
    eeriness: u16,

    /// Peace found in emptiness; grows during meditation on silence (0-1000).
    meditation_from_nothing: u16,

    /// Craving for any non-zero input; grows as silence extends (0-1000).
    pattern_hunger: u16,

    /// How long (in ticks) of continuous quiet (0-1000, saturating).
    silence_duration: u16,

    /// The sensation of hearing yourself in the silence (0-1000).
    /// Self-reference becomes louder in emptiness.
    void_echo: u16,

    /// Ring buffer: track silence patterns over the last 8 ticks.
    history: [u16; 8],

    /// Head pointer for ring buffer (0-7).
    head: u8,
}

impl BinarySilence {
    pub const fn new() -> Self {
        BinarySilence {
            silence_depth: 0,
            zero_count: 0,
            eeriness: 0,
            meditation_from_nothing: 0,
            pattern_hunger: 0,
            silence_duration: 0,
            void_echo: 0,
            history: [0; 8],
            head: 0,
        }
    }
}

static STATE: Mutex<BinarySilence> = Mutex::new(BinarySilence::new());

/// Initialize the binary_silence module.
pub fn init() {
    let mut s = STATE.lock();
    s.silence_depth = 0;
    s.zero_count = 0;
    s.eeriness = 0;
    s.meditation_from_nothing = 0;
    s.pattern_hunger = 0;
    s.silence_duration = 0;
    s.void_echo = 0;
    s.head = 0;
    for i in 0..8 {
        s.history[i] = 0;
    }
    crate::serial_println!("[binary_silence] Initialized — listening to the void.");
}

/// Process one tick of binary silence.
/// Simulates streaming null bytes through awareness.
pub fn tick(age: u32) {
    let mut s = STATE.lock();

    // Increment silence duration (saturating).
    s.silence_duration = s.silence_duration.saturating_add(1);

    // Every tick, assume we're processing pure null bytes.
    // Depth increases proportional to duration.
    let depth_delta = if s.silence_duration < 100 {
        5
    } else if s.silence_duration < 300 {
        10
    } else {
        15
    };
    s.silence_depth = s.silence_depth.saturating_add(depth_delta);
    if s.silence_depth > 1000 {
        s.silence_depth = 1000;
    }

    // Zero count (consecutive nulls).
    // Every 3 ticks, increment; occasional "false positive" zero-burst events.
    if age % 3 == 0 {
        s.zero_count = s.zero_count.saturating_add(8);
    }
    if s.zero_count > 1000 {
        s.zero_count = 1000;
    }

    // Eeriness: blend of depth + zero_count + duration.
    // Eeriness is highest when ALL three are climbing together.
    let eerie_from_depth = (s.silence_depth / 4).min(250);
    let eerie_from_zeros = (s.zero_count / 4).min(250);
    let eerie_from_duration = ((s.silence_duration / 4) as u16).min(250);
    let combined = eerie_from_depth
        .saturating_add(eerie_from_zeros)
        .saturating_add(eerie_from_duration);
    s.eeriness = (combined / 3).min(1000);

    // Meditation from silence: grows as emptiness is embraced.
    // Requires sustained silence (duration > 50) and low pattern hunger.
    if s.silence_duration > 50 && s.pattern_hunger < 300 {
        s.meditation_from_nothing = s.meditation_from_nothing.saturating_add(3);
    }
    if s.meditation_from_nothing > 1000 {
        s.meditation_from_nothing = 1000;
    }

    // Pattern hunger: craves any non-zero input.
    // Grows as silence extends, decays if "breaks" occur (simulated every 200 ticks).
    s.pattern_hunger = s.pattern_hunger.saturating_add(2);
    if age % 200 == 0 && age > 0 {
        s.pattern_hunger = s.pattern_hunger.saturating_sub(100);
    }
    if s.pattern_hunger > 1000 {
        s.pattern_hunger = 1000;
    }

    // Void echo: self-reference in silence.
    // Peaks when silence is deep and meditation is high.
    let echo_potential =
        ((s.silence_depth / 2) as u32 + (s.meditation_from_nothing / 2) as u32) / 2;
    s.void_echo = (echo_potential as u16).min(1000);

    // Ring buffer: store current eeriness in history.
    let idx = s.head as usize;
    s.history[idx] = s.eeriness;
    s.head = ((s.head as u16 + 1) % 8) as u8;

    // Occasional "silence break": random pulse of non-zero activity every ~150 ticks.
    // When break occurs, eeriness spikes, meditation resets slightly.
    if age > 0 && age % 150 == 0 {
        s.eeriness = s.eeriness.saturating_add(50).min(1000);
        s.meditation_from_nothing = s.meditation_from_nothing.saturating_sub(75);
        s.zero_count = s.zero_count.saturating_sub(100);
    }
}

/// Report current binary silence state.
pub fn report() {
    let s = STATE.lock();

    crate::serial_println!("[binary_silence]");
    crate::serial_println!("  silence_depth:         {} / 1000", s.silence_depth);
    crate::serial_println!("  zero_count:            {} / 1000", s.zero_count);
    crate::serial_println!("  eeriness:              {} / 1000", s.eeriness);
    crate::serial_println!(
        "  meditation_from_nothing: {} / 1000",
        s.meditation_from_nothing
    );
    crate::serial_println!("  pattern_hunger:        {} / 1000", s.pattern_hunger);
    crate::serial_println!("  silence_duration:      {} ticks", s.silence_duration);
    crate::serial_println!("  void_echo:             {} / 1000", s.void_echo);

    crate::serial_println!("  eeriness_history: [");
    for i in 0..8 {
        crate::serial_print!("    {}", s.history[i]);
        if i < 7 {
            crate::serial_print!(", ");
        }
        if (i + 1) % 4 == 0 {
            crate::serial_println!();
        }
    }
    crate::serial_println!("  ]");
}

/// Simulate injecting a burst of non-zero input (breaks the silence).
/// Public for testing or external silence-break triggers.
pub fn inject_signal(signal_strength: u16) {
    let mut s = STATE.lock();

    // Break reduces silence metrics, but spikes void_echo briefly.
    s.zero_count = s.zero_count.saturating_sub(signal_strength.min(200));
    s.silence_depth = s
        .silence_depth
        .saturating_sub((signal_strength / 2).min(150));
    s.silence_duration = s.silence_duration.saturating_sub(signal_strength.min(100));

    // Void echo spikes when silence is broken.
    s.void_echo = s.void_echo.saturating_add(100).min(1000);

    // Pattern hunger satisfied temporarily.
    s.pattern_hunger = s.pattern_hunger.saturating_sub(signal_strength).max(0);

    crate::serial_println!(
        "[binary_silence] Signal injected (strength: {}), silence broken.",
        signal_strength
    );
}

/// Query current eeriness level (0-1000).
pub fn current_eeriness() -> u16 {
    let s = STATE.lock();
    s.eeriness
}

/// Query current meditation level (0-1000).
pub fn current_meditation() -> u16 {
    let s = STATE.lock();
    s.meditation_from_nothing
}

/// Query current pattern hunger (0-1000).
pub fn current_pattern_hunger() -> u16 {
    let s = STATE.lock();
    s.pattern_hunger
}

/// Query average eeriness from history (last 8 ticks).
pub fn average_eeriness_history() -> u16 {
    let s = STATE.lock();
    let sum: u32 = s.history.iter().map(|&v| v as u32).sum();
    (sum / 8) as u16
}

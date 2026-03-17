use super::{q16_div, q16_mul, Q16, Q16_HALF, Q16_ONE, Q16_ZERO};
use crate::sync::Mutex;
/// Wake word detection engine for Genesis OS
///
/// Detects a configurable wake word from streaming audio using:
///   - Frame energy computation (Q16 fixed-point)
///   - Circular audio buffer for sliding window analysis
///   - Template-based pattern matching against stored reference
///   - Confidence scoring with adjustable threshold
///
/// No floating point. All math is Q16 (i32, 16 fractional bits).
use crate::{serial_print, serial_println};
use alloc::vec::Vec;

/// Maximum number of samples held in the circular buffer
const CIRCULAR_BUFFER_CAPACITY: usize = 48000; // 1 second at 48 kHz
/// Number of energy bins kept for pattern analysis
const ENERGY_HISTORY_LEN: usize = 64;
/// Default confidence threshold (0.65 in Q16)
const DEFAULT_THRESHOLD: Q16 = 42598; // 0.65 * 65536
/// Timeout in frames before returning to Listening from Detected
const DETECTION_TIMEOUT_FRAMES: u64 = 150;
/// Minimum energy to consider a frame as speech (0.02 in Q16)
const SPEECH_ENERGY_FLOOR: Q16 = 1310;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// State machine for wake word detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeWordState {
    /// Passively listening for energy spikes
    Listening,
    /// Energy spike detected, analysing pattern
    Detected,
    /// Pattern matched, command capture in progress
    Processing,
    /// No match within timeout, returning to Listening
    Timeout,
}

/// A single chunk of audio data
#[derive(Clone)]
pub struct AudioFrame {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
    pub timestamp: u64,
}

/// Stored wake word reference template (energy envelope)
struct WakeWordTemplate {
    /// Energy envelope of the reference utterance (Q16 values)
    energy_envelope: Vec<Q16>,
    /// Length in energy bins
    length: usize,
    /// Hash of the wake word name for identification
    name_hash: u64,
}

/// The core wake word detection engine
pub struct WakeWordEngine {
    /// Circular sample buffer
    buffer: Vec<i16>,
    /// Write position in the circular buffer
    write_pos: usize,
    /// Number of valid samples currently in buffer
    valid_samples: usize,
    /// Rolling energy history (Q16)
    energy_history: Vec<Q16>,
    /// Write position in energy history
    energy_write_pos: usize,
    /// Current detection state
    state: WakeWordState,
    /// Confidence of the most recent match (Q16, 0..Q16_ONE)
    confidence: Q16,
    /// Threshold above which we declare a match
    threshold: Q16,
    /// Frame counter since entering Detected state
    detect_counter: u64,
    /// Reference template for the active wake word
    template: Option<WakeWordTemplate>,
    /// Whether the engine has been triggered (latch)
    triggered: bool,
    /// Sample rate of incoming audio
    sample_rate: u32,
    /// Running average energy for adaptive thresholding (Q16)
    ambient_energy: Q16,
    /// Number of frames processed
    frames_processed: u64,
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

static ENGINE: Mutex<Option<WakeWordEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl WakeWordEngine {
    /// Create a new engine with default settings
    pub fn new(sample_rate: u32) -> Self {
        let mut buffer = Vec::new();
        buffer.resize(CIRCULAR_BUFFER_CAPACITY, 0i16);
        let mut energy_history = Vec::new();
        energy_history.resize(ENERGY_HISTORY_LEN, Q16_ZERO);

        WakeWordEngine {
            buffer,
            write_pos: 0,
            valid_samples: 0,
            energy_history,
            energy_write_pos: 0,
            state: WakeWordState::Listening,
            confidence: Q16_ZERO,
            threshold: DEFAULT_THRESHOLD,
            detect_counter: 0,
            template: None,
            triggered: false,
            sample_rate,
            ambient_energy: Q16_ZERO,
            frames_processed: 0,
        }
    }

    /// Feed a frame of audio samples into the engine and run detection.
    pub fn process_frame(&mut self, frame: &AudioFrame) {
        // Write samples into circular buffer
        for &s in &frame.samples {
            self.buffer[self.write_pos] = s;
            self.write_pos = (self.write_pos + 1) % CIRCULAR_BUFFER_CAPACITY;
        }
        let new_valid = self.valid_samples + frame.samples.len();
        self.valid_samples = if new_valid > CIRCULAR_BUFFER_CAPACITY {
            CIRCULAR_BUFFER_CAPACITY
        } else {
            new_valid
        };

        // Compute frame energy (Q16)
        let energy = compute_energy_q16(&frame.samples);

        // Store in energy history ring
        self.energy_history[self.energy_write_pos] = energy;
        self.energy_write_pos = (self.energy_write_pos + 1) % ENERGY_HISTORY_LEN;

        // Update ambient energy with exponential moving average
        // ambient = ambient * 0.98 + energy * 0.02
        let alpha = 64225; // 0.98 in Q16
        let beta = 1310; // 0.02 in Q16
        self.ambient_energy = q16_mul(self.ambient_energy, alpha) + q16_mul(energy, beta);

        self.frames_processed = self.frames_processed.saturating_add(1);

        // State machine
        match self.state {
            WakeWordState::Listening => {
                // Check if energy exceeds ambient + speech floor
                let activation_level = self.ambient_energy + SPEECH_ENERGY_FLOOR;
                if energy > activation_level && energy > SPEECH_ENERGY_FLOOR {
                    self.state = WakeWordState::Detected;
                    self.detect_counter = 0;
                    self.confidence = Q16_ZERO;
                }
            }
            WakeWordState::Detected => {
                self.detect_counter = self.detect_counter.saturating_add(1);

                // Run pattern matching against the template
                if let Some(ref tmpl) = self.template {
                    self.confidence = self.match_template(tmpl);
                    if self.confidence >= self.threshold {
                        self.state = WakeWordState::Processing;
                        self.triggered = true;
                        serial_println!(
                            "    [wake_word] Triggered (confidence: {})",
                            self.confidence >> 10 // rough percentage display
                        );
                    }
                } else {
                    // No template — use simple energy-burst heuristic
                    self.confidence = self.heuristic_confidence(energy);
                    if self.confidence >= self.threshold {
                        self.state = WakeWordState::Processing;
                        self.triggered = true;
                    }
                }

                if self.detect_counter > DETECTION_TIMEOUT_FRAMES {
                    self.state = WakeWordState::Timeout;
                }
            }
            WakeWordState::Processing => {
                // Remain in Processing until explicitly reset
            }
            WakeWordState::Timeout => {
                // Auto-return to Listening
                self.state = WakeWordState::Listening;
                self.confidence = Q16_ZERO;
            }
        }
    }

    /// Returns true if the wake word has been detected and not yet reset.
    pub fn is_triggered(&self) -> bool {
        self.triggered
    }

    /// Load a wake word reference from an energy envelope.
    /// `name_hash` — FNV-style hash of the wake word text.
    /// `envelope` — sequence of Q16 energy values from a reference recording.
    pub fn set_wake_word(&mut self, name_hash: u64, envelope: Vec<Q16>) {
        let length = envelope.len();
        self.template = Some(WakeWordTemplate {
            energy_envelope: envelope,
            length,
            name_hash,
        });
        serial_println!(
            "    [wake_word] Template set (hash: {:#X}, bins: {})",
            name_hash,
            length
        );
    }

    /// Reset the engine to the Listening state and clear the trigger latch.
    pub fn reset(&mut self) {
        self.triggered = false;
        self.state = WakeWordState::Listening;
        self.confidence = Q16_ZERO;
        self.detect_counter = 0;
    }

    /// Return the current confidence value (Q16, 0..Q16_ONE).
    pub fn get_confidence(&self) -> Q16 {
        self.confidence
    }

    /// Return the current detection state.
    pub fn get_state(&self) -> WakeWordState {
        self.state
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Cross-correlate recent energy history with stored template.
    /// Returns Q16 confidence in [0, Q16_ONE].
    fn match_template(&self, tmpl: &WakeWordTemplate) -> Q16 {
        if tmpl.length == 0 {
            return Q16_ZERO;
        }

        let hist_len = ENERGY_HISTORY_LEN;
        if tmpl.length > hist_len {
            return Q16_ZERO;
        }

        // Compute normalised cross-correlation over the most recent `tmpl.length` bins
        let start = if self.energy_write_pos >= tmpl.length {
            self.energy_write_pos - tmpl.length
        } else {
            hist_len - (tmpl.length - self.energy_write_pos)
        };

        let mut dot: i64 = 0;
        let mut norm_hist: i64 = 0;
        let mut norm_tmpl: i64 = 0;

        for i in 0..tmpl.length {
            let h = self.energy_history[(start + i) % hist_len] as i64;
            let t = tmpl.energy_envelope[i] as i64;
            dot += h * t;
            norm_hist += h * h;
            norm_tmpl += t * t;
        }

        // Avoid division by zero
        if norm_hist == 0 || norm_tmpl == 0 {
            return Q16_ZERO;
        }

        // Approximate sqrt via integer Newton's method for the denominator
        let denom_sq = norm_hist * norm_tmpl;
        let denom = isqrt_i64(denom_sq);
        if denom == 0 {
            return Q16_ZERO;
        }

        // correlation = dot / denom, scaled to Q16
        let corr = ((dot << 16) / denom) as Q16;

        // Clamp to [0, Q16_ONE]
        if corr < Q16_ZERO {
            Q16_ZERO
        } else if corr > Q16_ONE {
            Q16_ONE
        } else {
            corr
        }
    }

    /// Simple heuristic confidence when no template is loaded.
    /// Uses energy ratio above ambient.
    fn heuristic_confidence(&self, current_energy: Q16) -> Q16 {
        if self.ambient_energy <= 0 {
            return Q16_HALF;
        }
        // confidence = clamp((energy / ambient - 1) / 4, 0, 1) in Q16
        let ratio = q16_div(current_energy, self.ambient_energy); // Q16
        let excess = ratio - Q16_ONE; // how much above ambient
        if excess <= 0 {
            return Q16_ZERO;
        }
        // Scale: divide by 4 (shift right 2) to normalise
        let scaled = excess >> 2;
        if scaled > Q16_ONE {
            Q16_ONE
        } else {
            scaled
        }
    }
}

// ---------------------------------------------------------------------------
// Integer square root (Newton's method for i64)
// ---------------------------------------------------------------------------

fn isqrt_i64(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    if n == 1 {
        return 1;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

// ---------------------------------------------------------------------------
// Public free functions (operate on global ENGINE)
// ---------------------------------------------------------------------------

/// Compute the RMS energy of a slice of i16 samples, returned as Q16.
pub fn compute_energy(samples: &[i16]) -> Q16 {
    compute_energy_q16(samples)
}

fn compute_energy_q16(samples: &[i16]) -> Q16 {
    if samples.is_empty() {
        return Q16_ZERO;
    }
    let mut sum: i64 = 0;
    for &s in samples {
        let v = s as i64;
        sum += v * v;
    }
    let mean = sum / samples.len() as i64;
    // RMS = sqrt(mean), then scale to Q16
    let rms = isqrt_i64(mean);
    // Normalise: max i16 RMS is ~23170 (sine wave). Map to Q16_ONE at that level.
    // rms_q16 = rms * 65536 / 23170
    let rms_q16 = (rms << 16) / 23170;
    rms_q16 as Q16
}

/// Feed raw audio data into the global wake word engine.
pub fn feed_audio(samples: &[i16], sample_rate: u32, timestamp: u64) {
    let frame = AudioFrame {
        samples: Vec::from(samples),
        sample_rate,
        timestamp,
    };
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.process_frame(&frame);
    }
}

/// Check whether the global engine has triggered.
pub fn is_triggered() -> bool {
    let guard = ENGINE.lock();
    guard.as_ref().map(|e| e.is_triggered()).unwrap_or(false)
}

/// Reset the global engine to Listening state.
pub fn reset() {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.reset();
    }
}

/// Get the current confidence of the global engine (Q16).
pub fn get_confidence() -> Q16 {
    let guard = ENGINE.lock();
    guard
        .as_ref()
        .map(|e| e.get_confidence())
        .unwrap_or(Q16_ZERO)
}

/// Set the wake word template on the global engine.
pub fn set_wake_word(name_hash: u64, envelope: Vec<Q16>) {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.set_wake_word(name_hash, envelope);
    }
}

/// Get the current detection state of the global engine.
pub fn get_state() -> WakeWordState {
    let guard = ENGINE.lock();
    guard
        .as_ref()
        .map(|e| e.get_state())
        .unwrap_or(WakeWordState::Listening)
}

/// Initialize the wake word detection engine.
pub fn init() {
    let engine = WakeWordEngine::new(48000);
    *ENGINE.lock() = Some(engine);
    serial_println!(
        "    [wake_word] Wake word engine initialized (48 kHz, threshold: {})",
        DEFAULT_THRESHOLD
    );
}

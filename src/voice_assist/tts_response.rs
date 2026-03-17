use super::{q16_div, q16_from_int, q16_mul, Q16, Q16_HALF, Q16_ONE, Q16_ZERO};
use crate::sync::Mutex;
/// Text-to-speech response engine for Genesis OS
///
/// Generates audio output from response hashes and phoneme sequences:
///   - Phoneme-to-waveform synthesis (square/triangle wave approximation)
///   - Prosody control: pitch, speed, volume (Q16 fixed-point)
///   - Multiple voice presets (male, female, neutral)
///   - Utterance queue with priority ordering
///   - All synthesis is local, no external services
///
/// Audio output is a stream of i16 PCM samples at a configurable rate.
use crate::{serial_print, serial_println};
use alloc::vec;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default output sample rate
const DEFAULT_SAMPLE_RATE: u32 = 22050;
/// Maximum utterances in the queue
const MAX_UTTERANCE_QUEUE: usize = 16;
/// Number of built-in phonemes
const PHONEME_COUNT: usize = 44;
/// Default pitch (220 Hz as Q16)
const DEFAULT_PITCH: Q16 = 14417920; // 220 << 16
/// Default speed (1.0 in Q16)
const DEFAULT_SPEED: Q16 = Q16_ONE;
/// Samples per phoneme at default speed and rate (approx 80ms)
const SAMPLES_PER_PHONEME: usize = 1764; // 22050 * 0.08

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Voice gender
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceGender {
    Male,
    Female,
    Neutral,
}

/// A voice configuration
#[derive(Debug, Clone)]
pub struct TtsVoice {
    /// Unique voice identifier
    pub id: u32,
    /// Hash of the voice name
    pub name_hash: u64,
    /// Hash of the language identifier (e.g. "en-US")
    pub language_hash: u64,
    /// Base pitch in Hz (Q16)
    pub pitch: Q16,
    /// Playback speed multiplier (Q16, Q16_ONE = normal)
    pub speed: Q16,
    /// Gender classification
    pub gender: VoiceGender,
}

/// A single phoneme descriptor for waveform generation
#[derive(Debug, Clone, Copy)]
struct PhonemeDescriptor {
    /// Hash of the phoneme symbol (e.g. fnv1a of "AA")
    symbol_hash: u64,
    /// Relative frequency multiplier (Q16, Q16_ONE = base pitch)
    freq_factor: Q16,
    /// Relative amplitude (Q16, Q16_ONE = full volume)
    amplitude: Q16,
    /// Duration multiplier (Q16, Q16_ONE = default duration)
    duration_factor: Q16,
    /// Waveform type: 0 = square, 1 = triangle, 2 = pulse
    waveform: u8,
}

/// An utterance queued for synthesis
#[derive(Debug, Clone)]
pub struct Utterance {
    /// Sequence of phoneme indices into the phoneme table
    pub phoneme_indices: Vec<usize>,
    /// Priority (higher = speak sooner)
    pub priority: u8,
    /// Response hash this utterance corresponds to
    pub response_hash: u64,
}

/// The main TTS synthesis engine
pub struct TtsEngine {
    /// Active voice preset
    voice: TtsVoice,
    /// Available voice presets
    voices: Vec<TtsVoice>,
    /// Phoneme lookup table
    phoneme_table: Vec<PhonemeDescriptor>,
    /// Queue of utterances waiting to be synthesised
    queue: Vec<Utterance>,
    /// Audio output buffer (PCM i16)
    output_buffer: Vec<i16>,
    /// Read position in the output buffer
    read_pos: usize,
    /// Write position in the output buffer
    write_pos: usize,
    /// Whether the engine is currently producing audio
    speaking: bool,
    /// Output sample rate
    sample_rate: u32,
    /// Global volume (Q16, 0..Q16_ONE)
    volume: Q16,
    /// Phase accumulator for waveform generation (Q16)
    phase: Q16,
    /// Total utterances synthesised
    total_spoken: u64,
}

// ---------------------------------------------------------------------------
// Global instance
// ---------------------------------------------------------------------------

static ENGINE: Mutex<Option<TtsEngine>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl TtsEngine {
    /// Create a new TTS engine with default voice and phoneme table.
    pub fn new(sample_rate: u32) -> Self {
        let default_voice = TtsVoice {
            id: 1,
            name_hash: 0xABCDEF1234ABCDEF,
            language_hash: 0x1234ABCD1234ABCD,
            pitch: DEFAULT_PITCH,
            speed: DEFAULT_SPEED,
            gender: VoiceGender::Neutral,
        };

        let voices = vec![
            TtsVoice {
                id: 1,
                name_hash: 0xABCDEF1234ABCDEF,
                language_hash: 0x1234ABCD1234ABCD,
                pitch: DEFAULT_PITCH,
                speed: DEFAULT_SPEED,
                gender: VoiceGender::Neutral,
            },
            TtsVoice {
                id: 2,
                name_hash: 0xBBBBCCCC1111AAAA,
                language_hash: 0x1234ABCD1234ABCD,
                pitch: q16_from_int(130), // 130 Hz — deeper male
                speed: DEFAULT_SPEED,
                gender: VoiceGender::Male,
            },
            TtsVoice {
                id: 3,
                name_hash: 0xCCCCDDDD2222BBBB,
                language_hash: 0x1234ABCD1234ABCD,
                pitch: q16_from_int(300), // 300 Hz — higher female
                speed: DEFAULT_SPEED,
                gender: VoiceGender::Female,
            },
        ];

        let phoneme_table = build_phoneme_table();

        let mut output_buffer = Vec::new();
        output_buffer.resize(sample_rate as usize * 2, 0i16); // 2 seconds buffer

        TtsEngine {
            voice: default_voice,
            voices,
            phoneme_table,
            queue: Vec::new(),
            output_buffer,
            read_pos: 0,
            write_pos: 0,
            speaking: false,
            sample_rate,
            volume: Q16_ONE,
            phase: Q16_ZERO,
            total_spoken: 0,
        }
    }

    /// Synthesise and speak an utterance immediately (bypasses queue).
    pub fn speak(&mut self, utterance: Utterance) {
        self.generate_audio_for(&utterance);
        self.speaking = true;
        self.total_spoken = self.total_spoken.saturating_add(1);
    }

    /// Generate PCM audio for an utterance and append to the output buffer.
    pub fn generate_audio(&mut self) {
        if self.queue.is_empty() {
            self.speaking = false;
            return;
        }

        // Sort queue by priority (highest first)
        self.queue.sort_by(|a, b| b.priority.cmp(&a.priority));

        let utterance = self.queue.remove(0);
        self.generate_audio_for(&utterance);
        self.speaking = true;
        self.total_spoken = self.total_spoken.saturating_add(1);
    }

    /// Set the active voice by id.
    pub fn set_voice(&mut self, voice_id: u32) -> bool {
        for v in &self.voices {
            if v.id == voice_id {
                self.voice = v.clone();
                serial_println!("    [tts] Voice set to id={}", voice_id);
                return true;
            }
        }
        false
    }

    /// Adjust the playback speed (Q16 multiplier).
    pub fn set_speed(&mut self, speed: Q16) {
        let clamped = if speed < 16384 {
            16384 // min 0.25x
        } else if speed > q16_from_int(4) {
            q16_from_int(4) // max 4x
        } else {
            speed
        };
        self.voice.speed = clamped;
    }

    /// Adjust the base pitch (Q16, Hz << 16).
    pub fn set_pitch(&mut self, pitch: Q16) {
        let clamped = if pitch < q16_from_int(50) {
            q16_from_int(50)
        } else if pitch > q16_from_int(600) {
            q16_from_int(600)
        } else {
            pitch
        };
        self.voice.pitch = clamped;
    }

    /// Add an utterance to the queue.
    pub fn queue_utterance(&mut self, utterance: Utterance) {
        if self.queue.len() >= MAX_UTTERANCE_QUEUE {
            // Drop lowest priority
            let mut min_idx = 0;
            let mut min_pri = 255u8;
            for (i, u) in self.queue.iter().enumerate() {
                if u.priority < min_pri {
                    min_pri = u.priority;
                    min_idx = i;
                }
            }
            self.queue.remove(min_idx);
        }
        self.queue.push(utterance);

        // If not speaking, start synthesis
        if !self.speaking {
            self.generate_audio();
        }
    }

    /// Stop all synthesis and clear the queue.
    pub fn stop(&mut self) {
        self.queue.clear();
        self.speaking = false;
        self.read_pos = 0;
        self.write_pos = 0;
    }

    /// Whether the engine is currently producing audio.
    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Return a list of available voices.
    pub fn get_voices(&self) -> Vec<TtsVoice> {
        self.voices.clone()
    }

    /// Read synthesised PCM samples from the output buffer.
    /// Returns the number of samples actually read.
    pub fn read_samples(&mut self, dest: &mut [i16]) -> usize {
        let buf_len = self.output_buffer.len();
        let mut count = 0;

        for sample in dest.iter_mut() {
            if self.read_pos == self.write_pos {
                // Buffer exhausted
                if self.queue.is_empty() {
                    self.speaking = false;
                }
                break;
            }
            *sample = self.output_buffer[self.read_pos];
            self.read_pos = (self.read_pos + 1) % buf_len;
            count += 1;
        }

        count
    }

    /// Get the total number of utterances produced.
    pub fn get_total_spoken(&self) -> u64 {
        self.total_spoken
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Internal: synthesise audio for one utterance.
    fn generate_audio_for(&mut self, utterance: &Utterance) {
        let buf_len = self.output_buffer.len();
        let _base_pitch_hz = super::q16_to_int(self.voice.pitch) as u32;
        let speed_factor = self.voice.speed;

        for &idx in &utterance.phoneme_indices {
            if idx >= self.phoneme_table.len() {
                continue;
            }
            let phon = self.phoneme_table[idx];

            // Compute duration in samples
            let base_duration = SAMPLES_PER_PHONEME as i32;
            let duration_scaled = q16_mul(q16_from_int(base_duration), phon.duration_factor);
            // Adjust for speed: faster speed = fewer samples
            let speed_adjusted = if speed_factor > 0 {
                q16_div(duration_scaled, speed_factor)
            } else {
                duration_scaled
            };
            let num_samples = super::q16_to_int(speed_adjusted).max(1) as usize;

            // Compute frequency
            let freq_hz = q16_mul(self.voice.pitch, phon.freq_factor);
            let freq_int = super::q16_to_int(freq_hz).max(1) as u32;

            // Phase increment per sample (Q16)
            // phase_inc = (freq * 65536) / sample_rate
            let phase_inc = ((freq_int as i64 * Q16_ONE as i64) / self.sample_rate as i64) as Q16;

            for _ in 0..num_samples {
                // Generate waveform sample
                let raw = match phon.waveform {
                    0 => self.gen_square(phase_inc, phon.amplitude),
                    1 => self.gen_triangle(phase_inc, phon.amplitude),
                    2 => self.gen_pulse(phase_inc, phon.amplitude),
                    _ => self.gen_triangle(phase_inc, phon.amplitude),
                };

                // Apply global volume
                let scaled = q16_mul(q16_from_int(raw as i32), self.volume);
                let sample = super::q16_to_int(scaled) as i16;

                self.output_buffer[self.write_pos] = sample;
                self.write_pos = (self.write_pos + 1) % buf_len;
            }
        }
    }

    /// Generate a square wave sample and advance phase.
    fn gen_square(&mut self, phase_inc: Q16, amplitude: Q16) -> i16 {
        self.phase = (self.phase + phase_inc) % Q16_ONE;
        let amp = super::q16_to_int(amplitude) as i16;
        let max_amp = if amp > 16000 { 16000 } else { amp };
        if self.phase < Q16_HALF {
            max_amp
        } else {
            -max_amp
        }
    }

    /// Generate a triangle wave sample and advance phase.
    fn gen_triangle(&mut self, phase_inc: Q16, amplitude: Q16) -> i16 {
        self.phase = (self.phase + phase_inc) % Q16_ONE;
        let amp = super::q16_to_int(amplitude) as i32;
        let max_amp = if amp > 16000 { 16000 } else { amp };
        // Triangle: ramp from -amp to +amp in first half, +amp to -amp in second
        let sample = if self.phase < Q16_HALF {
            // First half: -amp to +amp
            let frac = q16_div(self.phase, Q16_HALF); // 0..Q16_ONE
            let val = -max_amp + (super::q16_to_int(q16_mul(q16_from_int(2 * max_amp), frac)));
            val
        } else {
            // Second half: +amp to -amp
            let frac = q16_div(self.phase - Q16_HALF, Q16_HALF);
            let val = max_amp - (super::q16_to_int(q16_mul(q16_from_int(2 * max_amp), frac)));
            val
        };
        sample as i16
    }

    /// Generate a narrow pulse wave sample and advance phase.
    fn gen_pulse(&mut self, phase_inc: Q16, amplitude: Q16) -> i16 {
        self.phase = (self.phase + phase_inc) % Q16_ONE;
        let amp = super::q16_to_int(amplitude) as i16;
        let max_amp = if amp > 16000 { 16000 } else { amp };
        // 25% duty cycle pulse
        let quarter = Q16_ONE >> 2;
        if self.phase < quarter {
            max_amp
        } else {
            -max_amp
        }
    }
}

// ---------------------------------------------------------------------------
// Phoneme table builder
// ---------------------------------------------------------------------------

/// Build the default phoneme descriptor table (44 English phonemes).
fn build_phoneme_table() -> Vec<PhonemeDescriptor> {
    let mut table = Vec::new();

    // Vowels — voiced, use triangle wave for smoother sound
    // Index 0: AA (as in "father")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D4,
        freq_factor: Q16_ONE,
        amplitude: q16_from_int(12000),
        duration_factor: 72089, // 1.1x
        waveform: 1,
    });
    // Index 1: AE (as in "cat")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D5,
        freq_factor: 70000, // slightly above 1.0
        amplitude: q16_from_int(11000),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 2: AH (as in "but")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D6,
        freq_factor: Q16_ONE,
        amplitude: q16_from_int(10000),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 3: AO (as in "dog")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D7,
        freq_factor: 58982, // 0.9x
        amplitude: q16_from_int(11500),
        duration_factor: 72089,
        waveform: 1,
    });
    // Index 4: AW (as in "how")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D8,
        freq_factor: Q16_ONE,
        amplitude: q16_from_int(11000),
        duration_factor: 78643, // 1.2x
        waveform: 1,
    });
    // Index 5: AY (as in "hide")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA1A1B2B2C3C3D4D9,
        freq_factor: 72089,
        amplitude: q16_from_int(11000),
        duration_factor: 78643,
        waveform: 1,
    });
    // Index 6: EH (as in "red")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA2A2B3B3C4C4D5DA,
        freq_factor: 72089,
        amplitude: q16_from_int(10500),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 7: ER (as in "bird")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA2A2B3B3C4C4D5DB,
        freq_factor: Q16_ONE,
        amplitude: q16_from_int(9500),
        duration_factor: 72089,
        waveform: 1,
    });
    // Index 8: EY (as in "say")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA2A2B3B3C4C4D5DC,
        freq_factor: 78643,
        amplitude: q16_from_int(10500),
        duration_factor: 78643,
        waveform: 1,
    });
    // Index 9: IH (as in "it")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA3A3B4B4C5C5D6DD,
        freq_factor: 78643,
        amplitude: q16_from_int(9000),
        duration_factor: 52428, // 0.8x
        waveform: 1,
    });
    // Index 10: IY (as in "eat")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA3A3B4B4C5C5D6DE,
        freq_factor: 85196, // ~1.3x
        amplitude: q16_from_int(10000),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 11: OW (as in "go")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA4A4B5B5C6C6D7DF,
        freq_factor: 58982,
        amplitude: q16_from_int(11000),
        duration_factor: 78643,
        waveform: 1,
    });
    // Index 12: OY (as in "boy")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA4A4B5B5C6C6D7EA,
        freq_factor: 62259,
        amplitude: q16_from_int(11000),
        duration_factor: 85196,
        waveform: 1,
    });
    // Index 13: UH (as in "book")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA5A5B6B6C7C7D8EB,
        freq_factor: 55705,
        amplitude: q16_from_int(9500),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 14: UW (as in "food")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xA5A5B6B6C7C7D8EC,
        freq_factor: 52428,
        amplitude: q16_from_int(10000),
        duration_factor: 72089,
        waveform: 1,
    });

    // Consonants — plosives (square wave, short duration)
    // Index 15: B
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB1B1C2C2D3D3E4ED,
        freq_factor: 45875, // 0.7x
        amplitude: q16_from_int(8000),
        duration_factor: 39321, // 0.6x
        waveform: 0,
    });
    // Index 16: D
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB1B1C2C2D3D3E4EE,
        freq_factor: 52428,
        amplitude: q16_from_int(7500),
        duration_factor: 39321,
        waveform: 0,
    });
    // Index 17: G
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB1B1C2C2D3D3E4EF,
        freq_factor: 45875,
        amplitude: q16_from_int(7000),
        duration_factor: 39321,
        waveform: 0,
    });
    // Index 18: K
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB2B2C3C3D4D4E5FA,
        freq_factor: 58982,
        amplitude: q16_from_int(6000),
        duration_factor: 32768, // 0.5x
        waveform: 2,
    });
    // Index 19: P
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB2B2C3C3D4D4E5FB,
        freq_factor: 55705,
        amplitude: q16_from_int(5500),
        duration_factor: 32768,
        waveform: 2,
    });
    // Index 20: T
    table.push(PhonemeDescriptor {
        symbol_hash: 0xB2B2C3C3D4D4E5FC,
        freq_factor: 62259,
        amplitude: q16_from_int(6000),
        duration_factor: 32768,
        waveform: 2,
    });

    // Fricatives (pulse wave, moderate duration)
    // Index 21: CH
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC1C1D2D2E3E3F4FD,
        freq_factor: 85196,
        amplitude: q16_from_int(5000),
        duration_factor: 52428,
        waveform: 2,
    });
    // Index 22: DH (as in "the")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC1C1D2D2E3E3F4FE,
        freq_factor: 45875,
        amplitude: q16_from_int(4500),
        duration_factor: 52428,
        waveform: 2,
    });
    // Index 23: F
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC2C2D3D3E4E4F5AF,
        freq_factor: 91750, // ~1.4x
        amplitude: q16_from_int(4000),
        duration_factor: Q16_ONE,
        waveform: 2,
    });
    // Index 24: HH
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC2C2D3D3E4E4F5BA,
        freq_factor: Q16_ONE,
        amplitude: q16_from_int(3000),
        duration_factor: 52428,
        waveform: 2,
    });
    // Index 25: JH (as in "judge")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC2C2D3D3E4E4F5BB,
        freq_factor: 55705,
        amplitude: q16_from_int(6000),
        duration_factor: 58982,
        waveform: 0,
    });
    // Index 26: S
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC3C3D4D4E5E5F6BC,
        freq_factor: 98304, // ~1.5x
        amplitude: q16_from_int(4500),
        duration_factor: Q16_ONE,
        waveform: 2,
    });
    // Index 27: SH
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC3C3D4D4E5E5F6BD,
        freq_factor: 91750,
        amplitude: q16_from_int(5000),
        duration_factor: 58982,
        waveform: 2,
    });
    // Index 28: TH (as in "think")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC3C3D4D4E5E5F6BE,
        freq_factor: 85196,
        amplitude: q16_from_int(3500),
        duration_factor: 58982,
        waveform: 2,
    });
    // Index 29: V
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC4C4D5D5E6E6F7BF,
        freq_factor: 55705,
        amplitude: q16_from_int(5500),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 30: Z
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC4C4D5D5E6E6F7CA,
        freq_factor: 91750,
        amplitude: q16_from_int(5000),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 31: ZH (as in "measure")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xC4C4D5D5E6E6F7CB,
        freq_factor: 85196,
        amplitude: q16_from_int(5500),
        duration_factor: 58982,
        waveform: 1,
    });

    // Nasals (triangle wave, medium duration)
    // Index 32: M
    table.push(PhonemeDescriptor {
        symbol_hash: 0xD1D1E2E2F3F3A4CC,
        freq_factor: 45875,
        amplitude: q16_from_int(8000),
        duration_factor: 72089,
        waveform: 1,
    });
    // Index 33: N
    table.push(PhonemeDescriptor {
        symbol_hash: 0xD1D1E2E2F3F3A4CD,
        freq_factor: 52428,
        amplitude: q16_from_int(7500),
        duration_factor: 72089,
        waveform: 1,
    });
    // Index 34: NG
    table.push(PhonemeDescriptor {
        symbol_hash: 0xD1D1E2E2F3F3A4CE,
        freq_factor: 45875,
        amplitude: q16_from_int(7000),
        duration_factor: 58982,
        waveform: 1,
    });

    // Liquids and glides (triangle wave)
    // Index 35: L
    table.push(PhonemeDescriptor {
        symbol_hash: 0xE1E1F2F2A3A3B4CF,
        freq_factor: 55705,
        amplitude: q16_from_int(8500),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 36: R
    table.push(PhonemeDescriptor {
        symbol_hash: 0xE1E1F2F2A3A3B4DA,
        freq_factor: 52428,
        amplitude: q16_from_int(8000),
        duration_factor: Q16_ONE,
        waveform: 1,
    });
    // Index 37: W
    table.push(PhonemeDescriptor {
        symbol_hash: 0xE1E1F2F2A3A3B4DB,
        freq_factor: 42598,
        amplitude: q16_from_int(9000),
        duration_factor: 58982,
        waveform: 1,
    });
    // Index 38: Y
    table.push(PhonemeDescriptor {
        symbol_hash: 0xE1E1F2F2A3A3B4DC,
        freq_factor: 72089,
        amplitude: q16_from_int(8500),
        duration_factor: 52428,
        waveform: 1,
    });

    // Special: silence / pause
    // Index 39: SIL (short pause)
    table.push(PhonemeDescriptor {
        symbol_hash: 0xF1F1A2A2B3B3C4DD,
        freq_factor: Q16_ZERO,
        amplitude: q16_from_int(0),
        duration_factor: Q16_HALF,
        waveform: 0,
    });
    // Index 40: PAU (long pause)
    table.push(PhonemeDescriptor {
        symbol_hash: 0xF1F1A2A2B3B3C4DE,
        freq_factor: Q16_ZERO,
        amplitude: q16_from_int(0),
        duration_factor: Q16_ONE,
        waveform: 0,
    });
    // Index 41: BREAK (sentence break)
    table.push(PhonemeDescriptor {
        symbol_hash: 0xF1F1A2A2B3B3C4DF,
        freq_factor: Q16_ZERO,
        amplitude: q16_from_int(0),
        duration_factor: 98304, // 1.5x
        waveform: 0,
    });

    // Affricates
    // Index 42: TS (as in "cats")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xF2F2A3A3B4B4C5EA,
        freq_factor: 91750,
        amplitude: q16_from_int(5000),
        duration_factor: 52428,
        waveform: 2,
    });
    // Index 43: DZ (as in "adds")
    table.push(PhonemeDescriptor {
        symbol_hash: 0xF2F2A3A3B4B4C5EB,
        freq_factor: 72089,
        amplitude: q16_from_int(5500),
        duration_factor: 52428,
        waveform: 0,
    });

    table
}

// ---------------------------------------------------------------------------
// Public free functions (operate on global ENGINE)
// ---------------------------------------------------------------------------

/// Speak an utterance immediately using the global TTS engine.
pub fn speak(utterance: Utterance) {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.speak(utterance);
    }
}

/// Generate the next batch of audio from the queue.
pub fn generate_audio() {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.generate_audio();
    }
}

/// Set the active voice by id.
pub fn set_voice(voice_id: u32) -> bool {
    let mut guard = ENGINE.lock();
    guard
        .as_mut()
        .map(|e| e.set_voice(voice_id))
        .unwrap_or(false)
}

/// Set the playback speed (Q16 multiplier).
pub fn set_speed(speed: Q16) {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.set_speed(speed);
    }
}

/// Set the base pitch (Q16, Hz << 16).
pub fn set_pitch(pitch: Q16) {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.set_pitch(pitch);
    }
}

/// Add an utterance to the synthesis queue.
pub fn queue_utterance(utterance: Utterance) {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.queue_utterance(utterance);
    }
}

/// Stop all synthesis and clear the queue.
pub fn stop() {
    let mut guard = ENGINE.lock();
    if let Some(ref mut engine) = *guard {
        engine.stop();
    }
}

/// Whether the engine is currently producing audio.
pub fn is_speaking() -> bool {
    let guard = ENGINE.lock();
    guard.as_ref().map(|e| e.is_speaking()).unwrap_or(false)
}

/// Get a list of available voices.
pub fn get_voices() -> Vec<TtsVoice> {
    let guard = ENGINE.lock();
    guard
        .as_ref()
        .map(|e| e.get_voices())
        .unwrap_or_else(Vec::new)
}

/// Read synthesised PCM samples from the output buffer.
pub fn read_samples(dest: &mut [i16]) -> usize {
    let mut guard = ENGINE.lock();
    guard.as_mut().map(|e| e.read_samples(dest)).unwrap_or(0)
}

/// Initialize the global TTS engine.
pub fn init() {
    let engine = TtsEngine::new(DEFAULT_SAMPLE_RATE);
    *ENGINE.lock() = Some(engine);
    serial_println!(
        "    [tts_response] TTS engine initialized ({} Hz, {} phonemes)",
        DEFAULT_SAMPLE_RATE,
        PHONEME_COUNT
    );
}

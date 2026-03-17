/// Voice AI for Genesis
///
/// Wake word detection, speech-to-text (ASR),
/// text-to-speech (TTS), speaker identification,
/// and voice command processing.
///
/// Inspired by: Apple Siri, Google Assistant, Whisper. All code is original.
use crate::sync::Mutex;
use alloc::string::String;
use alloc::vec::Vec;

/// Wake word state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeWordState {
    Listening,
    Detected,
    Cooldown,
    Disabled,
}

/// ASR (speech recognition) result
pub struct AsrResult {
    pub text: String,
    pub confidence: f32,
    pub is_final: bool,
    pub language: String,
    pub alternatives: Vec<(String, f32)>,
    pub duration_ms: u64,
}

/// TTS voice parameters
pub struct TtsVoice {
    pub name: String,
    pub language: String,
    pub gender: VoiceGender,
    pub speed: f32,  // 0.5 to 2.0
    pub pitch: f32,  // 0.5 to 2.0
    pub volume: f32, // 0.0 to 1.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceGender {
    Male,
    Female,
    Neutral,
}

/// Speaker profile for identification
pub struct SpeakerProfile {
    pub id: u32,
    pub name: String,
    pub voiceprint: Vec<f32>,
    pub enrolled_at: u64,
}

/// Voice command
pub struct VoiceCommand {
    pub text: String,
    pub intent: String,
    pub confidence: f32,
    pub slots: Vec<(String, String)>, // name -> value
}

// ---------------------------------------------------------------------------
// i32 fixed-point audio math helpers
// ---------------------------------------------------------------------------

/// Compute energy of an audio buffer as i64 (sum of squared samples).
fn compute_energy(audio: &[i16]) -> i64 {
    let mut energy: i64 = 0;
    for &s in audio {
        energy += (s as i64) * (s as i64);
    }
    energy
}

/// Compute zero-crossing rate (count of sign changes) over a buffer.
/// Returns the count as i32.
fn zero_crossing_count(audio: &[i16]) -> i32 {
    if audio.len() < 2 {
        return 0;
    }
    let mut count: i32 = 0;
    for i in 1..audio.len() {
        let prev_sign = audio[i - 1] >= 0;
        let curr_sign = audio[i] >= 0;
        if prev_sign != curr_sign {
            count += 1;
        }
    }
    count
}

/// Compute RMS amplitude in i32 (integer square root of mean squared).
fn rms_i32(audio: &[i16]) -> i32 {
    if audio.is_empty() {
        return 0;
    }
    let energy = compute_energy(audio);
    let mean_sq = energy / audio.len() as i64;
    isqrt_i64(mean_sq) as i32
}

/// Integer square root via Newton's method (i64 -> i64)
fn isqrt_i64(val: i64) -> i64 {
    if val <= 0 {
        return 0;
    }
    if val == 1 {
        return 1;
    }
    let mut guess = val / 2;
    let mut i = 0;
    while i < 30 {
        let next = (guess + val / guess) / 2;
        if next >= guess {
            break;
        }
        guess = next;
        i += 1;
    }
    guess
}

/// Simple cross-correlation between two i16 buffers.
/// Returns a normalized score in range [0, 1000] (i32 fixed scale).
fn cross_correlate(a: &[i16], b: &[i16]) -> i32 {
    let len = if a.len() < b.len() { a.len() } else { b.len() };
    if len == 0 {
        return 0;
    }
    let mut dot: i64 = 0;
    let mut norm_a: i64 = 0;
    let mut norm_b: i64 = 0;
    for i in 0..len {
        dot += a[i] as i64 * b[i] as i64;
        norm_a += a[i] as i64 * a[i] as i64;
        norm_b += b[i] as i64 * b[i] as i64;
    }
    let denom = isqrt_i64(norm_a) * isqrt_i64(norm_b);
    if denom == 0 {
        return 0;
    }
    // Scale to [0, 1000]
    ((dot * 1000) / denom) as i32
}

/// Compute energy in a frequency band using a simple DFT-like bin.
/// Rather than a full FFT, we compute the magnitude of a single
/// frequency bin via Goertzel's algorithm using integer math.
/// Returns energy as i64.
fn goertzel_energy(audio: &[i16], sample_rate: i32, target_freq: i32) -> i64 {
    let n = audio.len() as i32;
    if n == 0 {
        return 0;
    }
    // coeff = 2 * cos(2*pi*freq/sample_rate)
    // We precompute an integer approximation (scaled by 1024).
    // cos(2*pi*f/sr) ~ 1 - 2*(pi*f/sr)^2 for small f/sr (Taylor approx).
    // For more accuracy we use a lookup table of a few key ratios.
    let ratio_1000 = (target_freq as i64 * 1000) / sample_rate as i64; // f/sr * 1000
                                                                       // cos(2*pi*x) for x = ratio/1000, scaled by 1024
    let cos_scaled = cos_approx_1024(ratio_1000 as i32);
    let coeff = 2 * cos_scaled; // scaled by 1024

    let mut s0: i64 = 0;
    let mut s1: i64 = 0;
    let mut s2: i64;
    for &sample in audio {
        s2 = s1;
        s1 = s0;
        s0 = (sample as i64 * 1024) + ((coeff as i64 * s1) >> 10) - s2;
    }

    // Power = s1^2 + s0^2 - coeff*s0*s1 (all scaled)
    let power = (s1 * s1 + s0 * s0 - ((coeff as i64 * s0 * s1) >> 10)) >> 20;
    if power < 0 {
        0
    } else {
        power
    }
}

/// Integer approximation of cos(2*pi*x) * 1024, where x = ratio/1000.
/// Uses a quadratic approximation: cos(2*pi*x) ~ 1 - 2*(pi*x)^2 for small x.
/// Clamps to [-1024, 1024].
fn cos_approx_1024(ratio: i32) -> i32 {
    // x is ratio/1000, so 2*pi*x ~ 6283 * ratio / 1000
    // (2*pi*x)^2 * 1024 / 2 ~ (6283^2 * ratio^2 * 1024) / (1000^2 * 2)
    // Simplify: 6283^2 = 39476089; /2000000 * 1024 ~ 20219 * ratio^2 / 1000000
    let r = ratio as i64;
    let val = 1024 - ((20219 * r * r) / 1_000_000) as i32;
    if val > 1024 {
        1024
    } else if val < -1024 {
        -1024
    } else {
        val
    }
}

// ---------------------------------------------------------------------------
// Phoneme classification from frame features
// ---------------------------------------------------------------------------

/// Frame-level phoneme category
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhonemeClass {
    Silence,
    Voiced,    // vowels, nasals
    Unvoiced,  // plosives
    Fricative, // s, f, sh, etc.
}

/// Classify a single frame based on energy and zero-crossing rate.
fn classify_frame(energy: i64, zcr: i32, frame_len: usize) -> PhonemeClass {
    let rms = isqrt_i64(if frame_len > 0 {
        energy / frame_len as i64
    } else {
        0
    });
    let zcr_rate = if frame_len > 1 {
        (zcr as i64 * 1000) / (frame_len as i64 - 1)
    } else {
        0
    };

    if rms < 50 {
        PhonemeClass::Silence
    } else if zcr_rate > 300 {
        PhonemeClass::Fricative
    } else if rms > 200 && zcr_rate < 150 {
        PhonemeClass::Voiced
    } else if rms > 80 {
        PhonemeClass::Unvoiced
    } else {
        PhonemeClass::Silence
    }
}

/// Map a phoneme sequence to a basic word candidate using simple pattern matching.
/// This is rudimentary: it maps common phoneme-class patterns to words.
fn phoneme_sequence_to_words(seq: &[PhonemeClass]) -> Vec<(String, f32)> {
    let mut candidates: Vec<(String, f32)> = Vec::new();
    if seq.is_empty() {
        return candidates;
    }

    // Compress sequence into runs: (class, length)
    let mut runs: Vec<(PhonemeClass, usize)> = Vec::new();
    let mut cur_class = seq[0];
    let mut cur_len = 1usize;
    for i in 1..seq.len() {
        if seq[i] == cur_class {
            cur_len += 1;
        } else {
            runs.push((cur_class, cur_len));
            cur_class = seq[i];
            cur_len = 1;
        }
    }
    runs.push((cur_class, cur_len));

    // Filter out silence runs at the edges
    while runs.first().map_or(false, |r| r.0 == PhonemeClass::Silence) {
        runs.remove(0);
    }
    while runs.last().map_or(false, |r| r.0 == PhonemeClass::Silence) {
        runs.pop();
    }

    let non_silence: Vec<&(PhonemeClass, usize)> = runs
        .iter()
        .filter(|r| r.0 != PhonemeClass::Silence)
        .collect();

    // Pattern matching for common structures
    let pattern_len = non_silence.len();
    if pattern_len == 0 {
        return candidates;
    }

    // Single voiced segment: likely a simple vowel sound or short word
    if pattern_len == 1 && non_silence[0].0 == PhonemeClass::Voiced {
        candidates.push((String::from("ah"), 0.3));
        candidates.push((String::from("oh"), 0.2));
    }

    // Fricative + Voiced: words like "say", "see", "so", "she"
    if pattern_len >= 2
        && non_silence[0].0 == PhonemeClass::Fricative
        && non_silence[1].0 == PhonemeClass::Voiced
    {
        candidates.push((String::from("see"), 0.3));
        candidates.push((String::from("say"), 0.25));
    }

    // Voiced + Fricative: words like "is", "as", "us"
    if pattern_len >= 2
        && non_silence[0].0 == PhonemeClass::Voiced
        && non_silence[1].0 == PhonemeClass::Fricative
    {
        candidates.push((String::from("is"), 0.3));
        candidates.push((String::from("as"), 0.2));
    }

    // Unvoiced + Voiced: "ba", "da", "pa", "go"
    if pattern_len >= 2
        && non_silence[0].0 == PhonemeClass::Unvoiced
        && non_silence[1].0 == PhonemeClass::Voiced
    {
        candidates.push((String::from("go"), 0.3));
        candidates.push((String::from("do"), 0.2));
    }

    // Voiced + Unvoiced + Voiced: "open", "ago", "about"
    if pattern_len >= 3
        && non_silence[0].0 == PhonemeClass::Voiced
        && non_silence[1].0 == PhonemeClass::Unvoiced
        && non_silence[2].0 == PhonemeClass::Voiced
    {
        candidates.push((String::from("open"), 0.3));
        candidates.push((String::from("hello"), 0.25));
    }

    // Longer patterns: likely multi-syllable words
    if pattern_len >= 4 {
        candidates.push((String::from("computer"), 0.2));
        candidates.push((String::from("genesis"), 0.2));
    }

    // Check for wake-word-like pattern: 2 voiced segments with a gap
    let voiced_count = non_silence
        .iter()
        .filter(|r| r.0 == PhonemeClass::Voiced)
        .count();
    if voiced_count >= 2 && pattern_len >= 3 {
        candidates.push((String::from("hey hoags"), 0.3));
    }

    // Always provide a generic fallback
    if candidates.is_empty() {
        candidates.push((String::from("[speech]"), 0.1));
    }

    candidates
}

// ---------------------------------------------------------------------------
// TTS synthesis helpers (integer sine + formant mapping)
// ---------------------------------------------------------------------------

/// Integer sine approximation. Input: angle in units of 1/4096 of a full cycle.
/// Returns value in range [-32767, 32767] (i16 scale).
fn sin_i32(phase: i32) -> i32 {
    // Normalize to [0, 4095]
    let p = ((phase % 4096) + 4096) % 4096;

    // Quadrant decomposition
    let (quadrant, x) = match p {
        0..=1023 => (0, p),
        1024..=2047 => (1, 2047 - p),
        2048..=3071 => (2, p - 2048),
        _ => (3, 4095 - p),
    };

    // Parabolic sine approx: sin(x) ~ (4x/T)(1 - x/T) for first quarter
    // x is in [0, 1023], quarter period = 1024
    // sin ~ 4 * x * (1024 - x) / (1024 * 1024) * 32767
    let x64 = x as i64;
    let val = ((4 * x64 * (1024 - x64)) * 32767) / (1024 * 1024);
    let val = val as i32;

    match quadrant {
        0 | 1 => val,
        _ => -val,
    }
}

/// Map a character to a base frequency and duration for formant synthesis.
/// Returns (frequency_hz, duration_samples_at_16khz).
fn char_to_formant(ch: u8) -> (i32, i32) {
    match ch {
        // Vowels: lower frequencies, longer duration
        b'a' | b'A' => (800, 1200),
        b'e' | b'E' => (600, 1000),
        b'i' | b'I' => (400, 1000),
        b'o' | b'O' => (500, 1200),
        b'u' | b'U' => (350, 1000),

        // Voiced consonants: mid frequencies, shorter
        b'b' | b'B' => (200, 400),
        b'd' | b'D' => (300, 400),
        b'g' | b'G' => (250, 400),
        b'l' | b'L' => (350, 600),
        b'm' | b'M' => (200, 800),
        b'n' | b'N' => (250, 700),
        b'r' | b'R' => (300, 600),
        b'v' | b'V' => (350, 400),
        b'w' | b'W' => (300, 500),
        b'y' | b'Y' => (350, 500),
        b'z' | b'Z' => (400, 400),

        // Unvoiced consonants: higher frequency noise-like
        b'f' | b'F' => (600, 300),
        b'h' | b'H' => (500, 300),
        b'k' | b'K' => (700, 300),
        b'p' | b'P' => (600, 250),
        b's' | b'S' => (900, 400),
        b't' | b'T' => (800, 250),

        // Other consonants
        b'c' | b'C' => (700, 300),
        b'j' | b'J' => (400, 400),
        b'q' | b'Q' => (700, 300),
        b'x' | b'X' => (800, 300),

        // Space: silence
        b' ' => (0, 800),

        // Punctuation: brief pause
        b'.' | b',' | b'!' | b'?' => (0, 1200),

        // Default
        _ => (400, 300),
    }
}

// ---------------------------------------------------------------------------
// Speaker voiceprint extraction
// ---------------------------------------------------------------------------

/// Number of frequency bands for voiceprint feature vector
const VOICEPRINT_BANDS: usize = 16;

/// Extract a simple voiceprint (energy distribution across frequency bands).
/// Returns a Vec<f32> of length VOICEPRINT_BANDS.
fn extract_voiceprint(audio: &[i16], sample_rate: i32) -> Vec<f32> {
    let mut features = Vec::with_capacity(VOICEPRINT_BANDS);
    let band_width = sample_rate / (VOICEPRINT_BANDS as i32 * 2);

    let mut total_energy: i64 = 0;
    let mut band_energies: Vec<i64> = Vec::with_capacity(VOICEPRINT_BANDS);

    for band in 0..VOICEPRINT_BANDS {
        let freq = (band as i32 + 1) * band_width;
        let e = goertzel_energy(audio, sample_rate, freq);
        band_energies.push(e);
        total_energy += e;
    }

    // Normalize each band by total energy
    for e in &band_energies {
        let val = if total_energy > 0 {
            (*e as f32) / (total_energy as f32)
        } else {
            0.0
        };
        features.push(val);
    }

    features
}

/// Cosine similarity between two voiceprint vectors (f32).
fn voiceprint_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = if a.len() < b.len() { a.len() } else { b.len() };
    if len == 0 {
        return 0.0;
    }
    let mut dot: f32 = 0.0;
    let mut norm_a: f32 = 0.0;
    let mut norm_b: f32 = 0.0;
    for i in 0..len {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = sqrt_f32(norm_a) * sqrt_f32(norm_b);
    if denom < 0.0001 {
        return 0.0;
    }
    dot / denom
}

/// Software sqrt for no_std (Newton-Raphson)
fn sqrt_f32(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let mut guess = x;
    let mut i = 0;
    while i < 15 {
        guess = (guess + x / guess) * 0.5;
        i += 1;
    }
    guess
}

// ---------------------------------------------------------------------------
// Voice AI engine
// ---------------------------------------------------------------------------

/// Voice AI engine
pub struct VoiceAI {
    pub wake_word: String,
    pub wake_state: WakeWordState,
    pub listening: bool,
    pub asr_language: String,
    pub tts_voice: TtsVoice,
    pub speaker_profiles: Vec<SpeakerProfile>,
    pub next_speaker_id: u32,
    pub command_history: Vec<VoiceCommand>,
    pub continuous_listening: bool,
    pub noise_reduction: bool,
    pub echo_cancellation: bool,
    pub vad_enabled: bool, // voice activity detection
    pub vad_threshold: f32,
    pub total_utterances: u64,
    /// Cached wake word template (energy profile for matching)
    wake_template: Vec<i16>,
}

impl VoiceAI {
    const fn new() -> Self {
        VoiceAI {
            wake_word: String::new(),
            wake_state: WakeWordState::Listening,
            listening: false,
            asr_language: String::new(),
            tts_voice: TtsVoice {
                name: String::new(),
                language: String::new(),
                gender: VoiceGender::Neutral,
                speed: 1.0,
                pitch: 1.0,
                volume: 0.8,
            },
            speaker_profiles: Vec::new(),
            next_speaker_id: 1,
            command_history: Vec::new(),
            continuous_listening: false,
            noise_reduction: true,
            echo_cancellation: true,
            vad_enabled: true,
            vad_threshold: 0.5,
            total_utterances: 0,
            wake_template: Vec::new(),
        }
    }

    pub fn set_wake_word(&mut self, word: &str) {
        self.wake_word = String::from(word);
        // Generate a synthetic template from the wake word text using TTS
        self.wake_template = self.synthesize(word);
    }

    pub fn start_listening(&mut self) {
        self.listening = true;
    }

    pub fn stop_listening(&mut self) {
        self.listening = false;
    }

    /// Process audio buffer for wake word detection.
    ///
    /// 1. Compute energy -- reject silent buffers.
    /// 2. Extract spectral features (zero-crossing rate, energy bands).
    /// 3. Cross-correlate against the wake word template.
    pub fn detect_wake_word(&mut self, audio: &[i16]) -> bool {
        if self.wake_state == WakeWordState::Disabled || audio.is_empty() {
            return false;
        }

        // Step 1: energy gate
        let rms = rms_i32(audio);
        if rms < 100 {
            // Too quiet to be speech
            return false;
        }

        // Step 2: check spectral characteristics
        let zcr = zero_crossing_count(audio);
        let zcr_rate = (zcr as i64 * 1000) / (audio.len() as i64);
        // Speech typically has ZCR rate between 50-300 per 1000 samples
        if zcr_rate < 20 || zcr_rate > 500 {
            return false;
        }

        // Step 3: cross-correlate with wake template
        if !self.wake_template.is_empty() {
            // Slide the template across the audio buffer
            let tpl_len = self.wake_template.len();
            if audio.len() >= tpl_len {
                let mut best_score: i32 = 0;
                // Check a few positions (start, middle, end) for efficiency
                let positions = [
                    0,
                    audio.len() / 4,
                    audio.len() / 2,
                    audio.len().saturating_sub(tpl_len),
                ];
                for &pos in &positions {
                    let end = pos + tpl_len;
                    if end <= audio.len() {
                        let score = cross_correlate(&audio[pos..end], &self.wake_template);
                        if score > best_score {
                            best_score = score;
                        }
                    }
                }
                // Threshold: correlation > 400 out of 1000
                if best_score > 400 {
                    self.wake_state = WakeWordState::Detected;
                    return true;
                }
            }
        }

        // Fallback: if no template, use energy + spectral heuristic.
        // Check that there are at least 2 voiced segments (like "Hey Hoags").
        let frame_size = 160; // 10ms at 16kHz
        let mut voiced_segments = 0i32;
        let mut was_voiced = false;
        let mut frame_start = 0;
        while frame_start + frame_size <= audio.len() {
            let frame = &audio[frame_start..frame_start + frame_size];
            let frame_energy = compute_energy(frame);
            let frame_zcr = zero_crossing_count(frame);
            let cls = classify_frame(frame_energy, frame_zcr, frame_size);
            let is_voiced = cls == PhonemeClass::Voiced;
            if is_voiced && !was_voiced {
                voiced_segments += 1;
            }
            was_voiced = is_voiced;
            frame_start += frame_size;
        }

        if voiced_segments >= 2 && rms > 500 {
            self.wake_state = WakeWordState::Detected;
            return true;
        }

        false
    }

    /// Perform speech recognition on audio buffer.
    ///
    /// Divides audio into frames, computes energy + ZCR per frame,
    /// classifies each frame into phoneme categories, and maps
    /// the phoneme sequence to word candidates.
    pub fn recognize_speech(&mut self, audio: &[i16]) -> AsrResult {
        self.total_utterances = self.total_utterances.saturating_add(1);

        if audio.is_empty() {
            return AsrResult {
                text: String::new(),
                confidence: 0.0,
                is_final: false,
                language: self.asr_language.clone(),
                alternatives: Vec::new(),
                duration_ms: 0,
            };
        }

        // Assume 16kHz sample rate
        let sample_rate = 16000i32;
        let frame_size = 160; // 10ms frames
        let total_frames = audio.len() / frame_size;

        // Extract per-frame features and classify
        let mut phoneme_seq: Vec<PhonemeClass> = Vec::with_capacity(total_frames);
        for f in 0..total_frames {
            let start = f * frame_size;
            let frame = &audio[start..start + frame_size];
            let e = compute_energy(frame);
            let zcr = zero_crossing_count(frame);
            phoneme_seq.push(classify_frame(e, zcr, frame_size));
        }

        // Map phoneme sequence to word candidates
        let candidates = phoneme_sequence_to_words(&phoneme_seq);

        let duration_ms = (audio.len() as u64 * 1000) / sample_rate as u64;

        if candidates.is_empty() {
            return AsrResult {
                text: String::new(),
                confidence: 0.0,
                is_final: true,
                language: self.asr_language.clone(),
                alternatives: Vec::new(),
                duration_ms,
            };
        }

        // Best candidate
        let best = &candidates[0];

        // Build alternatives from the rest
        let alternatives: Vec<(String, f32)> = candidates
            .iter()
            .skip(1)
            .map(|(t, c)| (t.clone(), *c))
            .collect();

        AsrResult {
            text: best.0.clone(),
            confidence: best.1,
            is_final: true,
            language: self.asr_language.clone(),
            alternatives,
            duration_ms,
        }
    }

    /// Synthesize speech from text using integer formant synthesis.
    ///
    /// Maps each character to a frequency/duration pair and generates
    /// PCM samples at 16kHz using integer sine approximation.
    pub fn synthesize(&self, text: &str) -> Vec<i16> {
        let sample_rate: i32 = 16000;
        let bytes = text.as_bytes();
        let mut output: Vec<i16> = Vec::new();

        // Scale speed: 1.0 = normal, 2.0 = double speed (half duration)
        // Convert speed float to integer scale factor (x1000)
        let speed_1000 = (self.tts_voice.speed * 1000.0) as i32;
        let speed_factor = if speed_1000 > 0 { speed_1000 } else { 1000 };

        // Volume scale (0..1 -> 0..32767)
        let vol = (self.tts_voice.volume * 32767.0) as i32;

        // Pitch multiplier (x1000)
        let pitch_1000 = (self.tts_voice.pitch * 1000.0) as i32;
        let pitch_factor = if pitch_1000 > 0 { pitch_1000 } else { 1000 };

        let mut phase: i32 = 0; // accumulated phase in 1/4096 units

        for &ch in bytes {
            let (base_freq, base_dur) = char_to_formant(ch);

            // Apply pitch and speed
            let freq = (base_freq as i64 * pitch_factor as i64 / 1000) as i32;
            let dur = (base_dur as i64 * 1000 / speed_factor as i64) as i32;

            if freq == 0 {
                // Silence
                for _ in 0..dur {
                    output.push(0);
                }
                phase = 0;
            } else {
                // Generate sine wave samples
                // phase_increment per sample = freq * 4096 / sample_rate
                let phase_inc = (freq as i64 * 4096 / sample_rate as i64) as i32;

                for _ in 0..dur {
                    let sample = sin_i32(phase);
                    // Scale by volume
                    let scaled = ((sample as i64 * vol as i64) >> 15) as i16;
                    output.push(scaled);
                    phase = (phase + phase_inc) % 4096;
                }
            }

            // Small inter-character silence (20 samples = 1.25ms)
            for _ in 0..20 {
                output.push(0);
            }
        }

        output
    }

    /// Enroll a speaker profile
    pub fn enroll_speaker(&mut self, name: &str, voiceprint: Vec<f32>) -> u32 {
        let id = self.next_speaker_id;
        self.next_speaker_id = self.next_speaker_id.saturating_add(1);
        self.speaker_profiles.push(SpeakerProfile {
            id,
            name: String::from(name),
            voiceprint,
            enrolled_at: crate::time::clock::unix_time(),
        });
        id
    }

    /// Identify speaker from audio by extracting a voiceprint and
    /// comparing against stored profiles using cosine similarity.
    pub fn identify_speaker(&self, audio: &[i16]) -> Option<u32> {
        if self.speaker_profiles.is_empty() || audio.is_empty() {
            return None;
        }

        let sample_rate = 16000;
        let query_print = extract_voiceprint(audio, sample_rate);

        let mut best_id: Option<u32> = None;
        let mut best_sim: f32 = 0.0;
        let threshold: f32 = 0.6; // minimum similarity to match

        for profile in &self.speaker_profiles {
            let sim = voiceprint_similarity(&query_print, &profile.voiceprint);
            if sim > best_sim && sim >= threshold {
                best_sim = sim;
                best_id = Some(profile.id);
            }
        }

        best_id
    }

    /// Parse a voice command from recognized text
    pub fn parse_command(&mut self, text: &str) -> VoiceCommand {
        let lower = text.to_lowercase();
        let mut slots = Vec::new();
        let intent;

        if lower.starts_with("open ") {
            intent = String::from("open_app");
            let app = text[5..].trim();
            slots.push((String::from("app"), String::from(app)));
        } else if lower.starts_with("call ") {
            intent = String::from("make_call");
            let contact = text[5..].trim();
            slots.push((String::from("contact"), String::from(contact)));
        } else if lower.starts_with("set timer") || lower.starts_with("set alarm") {
            intent = String::from("set_timer");
        } else if lower.starts_with("play ") {
            intent = String::from("play_media");
            let media = text[5..].trim();
            slots.push((String::from("media"), String::from(media)));
        } else if lower.contains("weather") {
            intent = String::from("get_weather");
        } else if lower.starts_with("navigate to ") || lower.starts_with("directions to ") {
            intent = String::from("navigate");
            let dest = text
                .split_whitespace()
                .skip(2)
                .collect::<Vec<_>>()
                .join(" ");
            slots.push((String::from("destination"), dest));
        } else if lower.starts_with("send message") || lower.starts_with("text ") {
            intent = String::from("send_message");
        } else {
            intent = String::from("general_query");
        }

        let cmd = VoiceCommand {
            text: String::from(text),
            intent: intent.clone(),
            confidence: 0.8,
            slots,
        };
        self.command_history.push(VoiceCommand {
            text: String::from(text),
            intent,
            confidence: 0.8,
            slots: Vec::new(),
        });
        cmd
    }
}

static VOICE: Mutex<VoiceAI> = Mutex::new(VoiceAI::new());

pub fn init() {
    let mut v = VOICE.lock();
    v.wake_word = String::from("Hey Hoags");
    v.asr_language = String::from("en-US");
    v.tts_voice.name = String::from("Hoags Default");
    v.tts_voice.language = String::from("en-US");
    // Generate wake word template after setting TTS params
    let template = v.synthesize("Hey Hoags");
    v.wake_template = template;
    crate::serial_println!("    [voice] Voice AI initialized (wake word, ASR, TTS, speaker ID)");
}
